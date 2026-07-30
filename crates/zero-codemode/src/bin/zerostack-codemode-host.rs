use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, BufWriter, Write},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};
use zero_codemode::{
    is_executable_file, locate_report, CapabilityDescriptor, Connector, ConnectorError,
    DiscoveryEnv, DispatchContext, GlobalRegistration, Host, HostError, HostLimits,
};

const PROTOCOL: &str = "zerostack-codemode-host/v1";
const MAX_CELLS: usize = 1;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    Execute {
        id: u64,
        cell_id: String,
        source: String,
        #[serde(default)]
        yield_ms: u64,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    Wait {
        id: u64,
        cell_id: String,
        #[serde(default)]
        yield_ms: u64,
    },
    Terminate {
        id: u64,
        cell_id: String,
    },
    DelegateResponse {
        delegate_id: u64,
        ok: bool,
        #[serde(default)]
        result: Option<Value>,
        #[serde(default)]
        error: Option<String>,
    },
    Shutdown {
        id: u64,
    },
}

enum Event {
    Input(ClientFrame),
    InputError(String),
    InputClosed,
    Delegate(DelegateCall),
    Complete {
        cell_id: String,
        outcome: CellOutcome,
    },
}
struct DelegateCall {
    cell_id: String,
    payload: Value,
    response: mpsc::Sender<Result<Value, String>>,
}
#[derive(Clone)]
enum CellOutcome {
    Result(Value),
    Error(String),
    Terminated,
}
struct Waiter {
    request_id: u64,
    deadline: Option<Instant>,
}
struct Cell {
    cancelled: Arc<AtomicBool>,
    outcome: Option<CellOutcome>,
    waiter: Option<Waiter>,
}
struct PendingDelegate {
    cell_id: String,
    response: mpsc::Sender<Result<Value, String>>,
}
struct SidecarConnector {
    cell_id: String,
    cancelled: Arc<AtomicBool>,
    events: mpsc::Sender<Event>,
}

impl Connector for SidecarConnector {
    fn call(
        &self,
        _: &CapabilityDescriptor,
        args_json: &str,
        context: DispatchContext,
    ) -> Result<String, ConnectorError> {
        let payload =
            serde_json::from_str(args_json).map_err(|e| ConnectorError::new(e.to_string()))?;
        let (response, receive) = mpsc::channel();
        self.events
            .send(Event::Delegate(DelegateCall {
                cell_id: self.cell_id.clone(),
                payload,
                response,
            }))
            .map_err(|_| ConnectorError::new("sidecar transport closed"))?;
        loop {
            if self.cancelled.load(Ordering::Relaxed) {
                return Err(ConnectorError::new("execution cancelled"));
            }
            if context.is_expired() {
                return Err(ConnectorError::new("wall-clock deadline exceeded"));
            }
            match receive.recv_timeout(context.remaining().min(Duration::from_millis(25))) {
                Ok(Ok(value)) => {
                    return serde_json::to_string(&value)
                        .map_err(|e| ConnectorError::new(e.to_string()))
                }
                Ok(Err(message)) => return Err(ConnectorError::new(message)),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(ConnectorError::new("delegate transport closed"))
                }
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if print_cli_metadata()? {
        return Ok(());
    }
    let (events, receive) = mpsc::channel();
    read_stdin(events.clone());
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    write_frame(
        &mut writer,
        &json!({"type":"ready","protocol":PROTOCOL,"version":1}),
    )?;
    let mut cells: HashMap<String, Cell> = HashMap::new();
    let mut delegates: HashMap<u64, PendingDelegate> = HashMap::new();
    let delegate_ids = AtomicU64::new(1);

    loop {
        flush_deadlines(&mut writer, &mut cells)?;
        let event = match next_deadline(&cells) {
            Some(timeout) => match receive.recv_timeout(timeout) {
                Ok(e) => e,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(_) => break,
            },
            None => match receive.recv() {
                Ok(e) => e,
                Err(_) => break,
            },
        };
        match event {
            Event::Input(frame) => match frame {
                ClientFrame::Execute {
                    id,
                    cell_id,
                    source,
                    yield_ms,
                    timeout_ms,
                } => {
                    if cells.contains_key(&cell_id) {
                        write_error(&mut writer, id, "cell already exists")?;
                        continue;
                    }
                    if cells.len() >= MAX_CELLS {
                        write_error(&mut writer, id, "cell capacity exhausted")?;
                        continue;
                    }
                    let cancelled = Arc::new(AtomicBool::new(false));
                    cells.insert(
                        cell_id.clone(),
                        Cell {
                            cancelled: Arc::clone(&cancelled),
                            outcome: None,
                            waiter: Some(Waiter {
                                request_id: id,
                                deadline: deadline(yield_ms),
                            }),
                        },
                    );
                    spawn_cell(cell_id, source, timeout_ms, cancelled, events.clone());
                }
                ClientFrame::Wait {
                    id,
                    cell_id,
                    yield_ms,
                } => {
                    let Some(cell) = cells.get_mut(&cell_id) else {
                        write_missing(&mut writer, id, &cell_id)?;
                        continue;
                    };
                    if cell.waiter.is_some() {
                        write_error(&mut writer, id, "cell already has a waiter")?;
                        continue;
                    }
                    if let Some(outcome) = cell.outcome.clone() {
                        write_outcome(&mut writer, id, &cell_id, outcome)?;
                        cells.remove(&cell_id);
                    } else {
                        cell.waiter = Some(Waiter {
                            request_id: id,
                            deadline: deadline(yield_ms),
                        });
                    }
                }
                ClientFrame::Terminate { id, cell_id } => {
                    if let Some(cell) = cells.get_mut(&cell_id) {
                        cell.cancelled.store(true, Ordering::Relaxed);
                        cell.waiter = Some(Waiter {
                            request_id: id,
                            deadline: Some(Instant::now() + Duration::from_secs(1)),
                        });
                        cancel_delegates(&cell_id, &mut delegates);
                    } else {
                        write_missing(&mut writer, id, &cell_id)?;
                    }
                }
                ClientFrame::DelegateResponse {
                    delegate_id,
                    ok,
                    result,
                    error,
                } => {
                    if let Some(pending) = delegates.remove(&delegate_id) {
                        let value = if ok {
                            Ok(result.unwrap_or(Value::Null))
                        } else {
                            Err(error.unwrap_or_else(|| "delegate failed".to_owned()))
                        };
                        let _ = pending.response.send(value);
                    }
                }
                ClientFrame::Shutdown { id } => {
                    for cell in cells.values() {
                        cell.cancelled.store(true, Ordering::Relaxed);
                    }
                    for pending in delegates.into_values() {
                        let _ = pending
                            .response
                            .send(Err("sidecar shutting down".to_owned()));
                    }
                    write_frame(&mut writer, &json!({"type":"response","id":id,"ok":true}))?;
                    break;
                }
            },
            Event::Delegate(call) => {
                if !cells.contains_key(&call.cell_id) {
                    let _ = call.response.send(Err("cell is unavailable".to_owned()));
                    continue;
                }
                let delegate_id = delegate_ids.fetch_add(1, Ordering::Relaxed);
                delegates.insert(
                    delegate_id,
                    PendingDelegate {
                        cell_id: call.cell_id.clone(),
                        response: call.response,
                    },
                );
                write_frame(
                    &mut writer,
                    &json!({"type":"delegate_request","delegate_id":delegate_id,"cell_id":call.cell_id,"payload":call.payload}),
                )?;
            }
            Event::Complete { cell_id, outcome } => {
                cancel_delegates(&cell_id, &mut delegates);
                let Some(cell) = cells.get_mut(&cell_id) else {
                    continue;
                };
                if let Some(waiter) = cell.waiter.take() {
                    write_outcome(&mut writer, waiter.request_id, &cell_id, outcome)?;
                    cells.remove(&cell_id);
                } else {
                    cell.outcome = Some(outcome);
                }
            }
            Event::InputError(message) => write_frame(
                &mut writer,
                &json!({"type":"protocol_error","error":message}),
            )?,
            Event::InputClosed => {
                for cell in cells.values() {
                    cell.cancelled.store(true, Ordering::Relaxed);
                }
                break;
            }
        }
    }
    Ok(())
}

fn print_cli_metadata() -> Result<bool, Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => Ok(false),
        [flag] if flag == "--version" || flag == "-V" => {
            println!(
                "zerostack-codemode-host {} ({PROTOCOL})",
                env!("CARGO_PKG_VERSION")
            );
            Ok(true)
        }
        [flag] if flag == "--help" || flag == "-h" => {
            println!("ZeroStack native aggregate CodeMode sidecar\n\nUsage: zerostack-codemode-host [--help|--version|--locate]\n\nWithout arguments, bounded NDJSON frames are read from stdin and written to stdout.\n\n--locate prints a JSON discovery report so a harness needs no absolute paths in\nits config. Resolution order: $ZEROSTACK_HOME/bin, $ZEROSTACK_DEV_ROOT/<Repo>/target/release,\n$XDG_DATA_HOME/zerostack/bin, platform install dirs, then PATH.");
            Ok(true)
        }
        [flag] if flag == "--locate" => {
            // Discovery is reported, never enforced here: a harness may legitimately
            // run with only the engines it installed, so unresolved delegates are
            // data in the report rather than a non-zero exit.
            let env = DiscoveryEnv::from_process();
            println!(
                "{}",
                serde_json::to_string_pretty(&locate_report(&env, &is_executable_file))?
            );
            Ok(true)
        }
        _ => Err(format!("unsupported arguments: {}", args.join(" ")).into()),
    }
}

fn read_stdin(events: mpsc::Sender<Event>) {
    thread::spawn(move || {
        let stdin = io::stdin();
        for line in BufReader::new(stdin.lock()).lines() {
            match line {
                Ok(line) if line.trim().is_empty() => continue,
                Ok(line) => match serde_json::from_str(&line) {
                    Ok(frame) => {
                        if events.send(Event::Input(frame)).is_err() {
                            return;
                        }
                    }
                    Err(e) => {
                        if events.send(Event::InputError(e.to_string())).is_err() {
                            return;
                        }
                    }
                },
                Err(e) => {
                    let _ = events.send(Event::InputError(e.to_string()));
                    break;
                }
            }
        }
        let _ = events.send(Event::InputClosed);
    });
}

fn spawn_cell(
    cell_id: String,
    source: String,
    timeout_ms: Option<u64>,
    cancelled: Arc<AtomicBool>,
    events: mpsc::Sender<Event>,
) {
    thread::spawn(move || {
        let execute = || -> Result<Value, HostError> {
            let limits = HostLimits::new(
                128 * 1024 * 1024,
                1024 * 1024,
                Duration::from_millis(timeout_ms.unwrap_or(3_600_000).clamp(1, 3_600_000)),
                10_000_000,
                16_384,
                256 * 1024,
                16 * 1024 * 1024,
            )
            .map_err(HostError::Limits)?;
            let host = Host::new(
                limits,
                GlobalRegistration {
                    root: "__zero".to_owned(),
                    capabilities: vec![CapabilityDescriptor::new("host", "call")],
                },
            )?;
            host.execute_with_cancel(
                &source,
                Rc::new(SidecarConnector {
                    cell_id: cell_id.clone(),
                    cancelled: Arc::clone(&cancelled),
                    events: events.clone(),
                }),
                cancelled,
            )
        };
        let outcome = match execute() {
            Ok(value) => CellOutcome::Result(value),
            Err(HostError::Cancelled) => CellOutcome::Terminated,
            Err(e) => CellOutcome::Error(e.to_string()),
        };
        let _ = events.send(Event::Complete { cell_id, outcome });
    });
}

fn deadline(yield_ms: u64) -> Option<Instant> {
    (yield_ms > 0).then(|| Instant::now() + Duration::from_millis(yield_ms))
}
fn next_deadline(cells: &HashMap<String, Cell>) -> Option<Duration> {
    cells
        .values()
        .filter_map(|cell| cell.waiter.as_ref()?.deadline)
        .min()
        .map(|d| d.saturating_duration_since(Instant::now()))
}
fn flush_deadlines(
    writer: &mut BufWriter<io::StdoutLock<'_>>,
    cells: &mut HashMap<String, Cell>,
) -> io::Result<()> {
    let now = Instant::now();
    let expired: Vec<_> = cells
        .iter()
        .filter_map(|(id, cell)| {
            cell.waiter.as_ref()?.deadline.filter(|d| *d <= now)?;
            Some(id.clone())
        })
        .collect();
    for cell_id in expired {
        if let Some(cell) = cells.get_mut(&cell_id) {
            if let Some(waiter) = cell.waiter.take() {
                if cell.cancelled.load(Ordering::Relaxed) {
                    write_outcome(writer, waiter.request_id, &cell_id, CellOutcome::Terminated)?;
                } else {
                    write_frame(
                        writer,
                        &json!({"type":"response","id":waiter.request_id,"ok":true,"kind":"yielded","cellId":cell_id,"contentItems":[]}),
                    )?;
                }
            }
        }
    }
    Ok(())
}
fn cancel_delegates(cell_id: &str, delegates: &mut HashMap<u64, PendingDelegate>) {
    let ids: Vec<_> = delegates
        .iter()
        .filter_map(|(id, p)| (p.cell_id == cell_id).then_some(*id))
        .collect();
    for id in ids {
        if let Some(p) = delegates.remove(&id) {
            let _ = p.response.send(Err("execution cancelled".to_owned()));
        }
    }
}
fn write_outcome(
    writer: &mut BufWriter<io::StdoutLock<'_>>,
    id: u64,
    cell_id: &str,
    outcome: CellOutcome,
) -> io::Result<()> {
    let frame = match outcome {
        CellOutcome::Result(value) => {
            let text = match value {
                Value::String(v) => v,
                v => serde_json::to_string(&v).unwrap_or_else(|_| "null".to_owned()),
            };
            json!({"type":"response","id":id,"ok":true,"kind":"result","cellId":cell_id,"contentItems":[{"type":"input_text","text":text}]})
        }
        CellOutcome::Error(message) => {
            json!({"type":"response","id":id,"ok":true,"kind":"result","cellId":cell_id,"errorText":message,"contentItems":[]})
        }
        CellOutcome::Terminated => {
            json!({"type":"response","id":id,"ok":true,"kind":"terminated","cellId":cell_id,"contentItems":[]})
        }
    };
    write_frame(writer, &frame)
}
fn write_missing(
    writer: &mut BufWriter<io::StdoutLock<'_>>,
    id: u64,
    cell_id: &str,
) -> io::Result<()> {
    write_frame(
        writer,
        &json!({"type":"response","id":id,"ok":true,"kind":"missing","cellId":cell_id,"missingCell":true,"contentItems":[]}),
    )
}
fn write_error(
    writer: &mut BufWriter<io::StdoutLock<'_>>,
    id: u64,
    message: &str,
) -> io::Result<()> {
    write_frame(
        writer,
        &json!({"type":"response","id":id,"ok":false,"error":message}),
    )
}
fn write_frame(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()
}
