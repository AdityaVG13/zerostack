use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    io::{self, BufRead, BufReader, BufWriter, Read, Write},
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};
use zero_codemode::node::{NODE_SCHEMA, NodeEnv, node_report};
use zero_codemode::session::{
    AggregateSession, AggregateSessionError, AggregateSessionFailureCode, MAX_SESSION_FRAME,
    SessionReplacementReason,
};
use zero_codemode::{
    ArtifactEnv, DISCOVERY_SCHEMA, DiscoveryEnv, MANIFEST_SCHEMA, ManifestFacts, StorePaths,
    finalize_visible_error, is_executable_file, is_readable_file, locate_manifest, locate_report,
};
use zero_store::{Engine, ResolvedStore};

/// v2 executes every plan directly on the aggregate session: capability calls
/// lower to raw-worker v2 children inside the host, so no delegate frames
/// cross this transport.
const PROTOCOL: &str = "zerostack-codemode-host/v2";
const MAX_CELLS: usize = 1;
const DEFAULT_EXECUTION_TIMEOUT_MS: u64 = 3_600_000;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ClientFrame {
    Execute {
        id: u64,
        cell_id: String,
        source: String,
        /// Generation reported by the ready frame. When present it must match
        /// the active session generation; a stale generation is rejected
        /// before admission.
        #[serde(default)]
        generation: Option<u64>,
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
    Shutdown {
        id: u64,
    },
}

enum Event {
    Input(ClientFrame),
    InputError(String),
    InputClosed,
    Complete {
        cell_id: String,
        outcome: CellOutcome,
        duration_ms: u64,
    },
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
    outcome: Option<CellOutcome>,
    duration_ms: Option<u64>,
    waiter: Option<Waiter>,
    started: Instant,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("zerostack-codemode-host: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    if print_cli_metadata()? {
        return Ok(());
    }
    // Fail-loud startup authorization: the aggregate session requires an
    // explicit ZEROSTACK_SESSION_ROOT and all three raw worker binaries, and
    // refuses to start until the executor prewarm succeeds. Nothing is emitted
    // until authorization passes.
    let session = Arc::new(AggregateSession::new(initial_generation())?);
    let mut generation = session.generation()?;
    let (events, receive) = mpsc::channel();
    read_stdin(events.clone());
    let stdout = io::stdout();
    let mut writer = BufWriter::new(stdout.lock());
    write_frame(
        &mut writer,
        &json!({"type":"ready","protocol":PROTOCOL,"version":2,"generation":generation}),
    )?;
    let mut cells: HashMap<String, Cell> = HashMap::new();

    loop {
        flush_deadlines(&mut writer, &mut cells, generation)?;
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
                    generation: requested,
                    yield_ms,
                    timeout_ms,
                } => {
                    if let Some(requested) = requested
                        && requested != generation
                    {
                        write_error(
                            &mut writer,
                            id,
                            generation,
                            &format!("stale generation: expected {generation}, got {requested}"),
                        )?;
                        continue;
                    }
                    if cells.contains_key(&cell_id) {
                        write_error(&mut writer, id, generation, "cell already exists")?;
                        continue;
                    }
                    if cells.len() >= MAX_CELLS {
                        write_error(&mut writer, id, generation, "cell capacity exhausted")?;
                        continue;
                    }
                    cells.insert(
                        cell_id.clone(),
                        Cell {
                            outcome: None,
                            duration_ms: None,
                            started: Instant::now(),
                            waiter: Some(Waiter {
                                request_id: id,
                                deadline: deadline(yield_ms),
                            }),
                        },
                    );
                    spawn_cell(
                        cell_id,
                        source,
                        timeout_ms,
                        generation,
                        id,
                        Arc::clone(&session),
                        events.clone(),
                    );
                }
                ClientFrame::Wait {
                    id,
                    cell_id,
                    yield_ms,
                } => {
                    let Some(cell) = cells.get_mut(&cell_id) else {
                        write_missing(&mut writer, id, &cell_id, generation)?;
                        continue;
                    };
                    if cell.waiter.is_some() {
                        write_error(&mut writer, id, generation, "cell already has a waiter")?;
                        continue;
                    }
                    if let Some(outcome) = cell.outcome.clone() {
                        let duration_ms = cell.duration_ms;
                        write_outcome(&mut writer, id, &cell_id, outcome, duration_ms, generation)?;
                        cells.remove(&cell_id);
                    } else {
                        cell.waiter = Some(Waiter {
                            request_id: id,
                            deadline: deadline(yield_ms),
                        });
                    }
                }
                ClientFrame::Terminate { id, cell_id } => {
                    if !cells.contains_key(&cell_id) {
                        write_missing(&mut writer, id, &cell_id, generation)?;
                        continue;
                    }
                    // Cancellation replacement: cancelling the backend rolls the
                    // generation forward and returns the session to accepting,
                    // so the next execute after a terminate is healthy.
                    match session.replace(generation, SessionReplacementReason::Manual) {
                        Ok(receipt) => {
                            generation = receipt.generation;
                            cells.remove(&cell_id);
                            write_outcome(
                                &mut writer,
                                id,
                                &cell_id,
                                CellOutcome::Terminated,
                                None,
                                generation,
                            )?;
                        }
                        Err(error) => {
                            write_error(
                                &mut writer,
                                id,
                                generation,
                                &format!("terminate failed: {error}"),
                            )?;
                        }
                    }
                }
                ClientFrame::Shutdown { id } => {
                    match session.shutdown() {
                        Ok(_) => {
                            write_frame(&mut writer, &json!({"type":"response","id":id,"ok":true}))?
                        }
                        Err(error) => write_error(
                            &mut writer,
                            id,
                            generation,
                            &format!("shutdown failed: {error}"),
                        )?,
                    }
                    break;
                }
            },
            Event::Complete {
                cell_id,
                outcome,
                duration_ms,
            } => {
                // The cell may already be gone: terminate removes it before the
                // replaced execution settles.
                let Some(cell) = cells.get_mut(&cell_id) else {
                    continue;
                };
                if let Some(waiter) = cell.waiter.take() {
                    write_outcome(
                        &mut writer,
                        waiter.request_id,
                        &cell_id,
                        outcome,
                        Some(duration_ms),
                        generation,
                    )?;
                    cells.remove(&cell_id);
                } else {
                    cell.outcome = Some(outcome);
                    cell.duration_ms = Some(duration_ms);
                }
            }
            Event::InputError(message) => write_frame(
                &mut writer,
                &json!({"type":"protocol_error","error":message}),
            )?,
            Event::InputClosed => {
                let _ = session.shutdown();
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
            println!(
                "ZeroStack native aggregate CodeMode sidecar\n\nUsage: zerostack-codemode-host [--help|--version|--locate [--json]|--locate-binaries|--locate-node]\n\nWithout arguments, bounded NDJSON frames are read from stdin and written to stdout.\nServe mode requires explicit ZEROSTACK_SESSION_ROOT authorization plus the\nZERO_FSZERO_RAW_BIN / ZERO_GRAPHZERO_RAW_BIN / ZERO_TOKENZERO_RAW_BIN worker pins;\nmissing authorization fails loudly before the ready frame.\n\n--locate prints every binary, module, and store path a harness needs, so no\nabsolute path belongs in its config; --json emits the {MANIFEST_SCHEMA} manifest.\n--locate-binaries emits the narrower {DISCOVERY_SCHEMA} executable report.\n--locate-node emits the {NODE_SCHEMA} node runtime report, which refuses\nper-shell fnm multishell paths so a pin cannot die with the shell that made it.\n\nResolution order: explicit pin, $ZEROSTACK_HOME, $ZEROSTACK_DEV_ROOT/<Repo>,\n$XDG_DATA_HOME/zerostack, platform install dirs, then PATH."
            );
            Ok(true)
        }
        [flag] if flag == "--locate-binaries" => {
            // Discovery is reported, never enforced here: a harness may legitimately
            // run with only the engines it installed, so unresolved engines are
            // data in the report rather than a non-zero exit.
            let env = DiscoveryEnv::from_process();
            println!(
                "{}",
                serde_json::to_string_pretty(&locate_report(&env, &is_executable_file))?
            );
            Ok(true)
        }
        [flag] if flag == "--locate-node" => {
            // Reported, never enforced: a harness that needs no JavaScript runtime
            // is a legitimate install, so an unresolved node is data in the report
            // rather than a non-zero exit.
            println!(
                "{}",
                serde_json::to_string_pretty(&node_report(
                    &NodeEnv::from_process(),
                    &is_executable_file
                ))?
            );
            Ok(true)
        }
        [flag] if flag == "--locate" => {
            print_manifest(false)?;
            Ok(true)
        }
        [locate, json] if locate == "--locate" && json == "--json" => {
            print_manifest(true)?;
            Ok(true)
        }
        _ => Err(format!("unsupported arguments: {}", args.join(" ")).into()),
    }
}

/// Print the harness manifest: JSON for machines, one aligned line per entry for
/// a human reading a terminal.
///
/// Unresolved entries are data, not failure: an install with only some engines is
/// legitimate, and the harness decides which engines it requires.
fn print_manifest(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let manifest = locate_manifest(
        &DiscoveryEnv::from_process(),
        &ArtifactEnv::from_process(),
        &ManifestFacts {
            host_version: env!("CARGO_PKG_VERSION").to_owned(),
            protocol: PROTOCOL.to_owned(),
            store: store_paths(),
        },
        &is_executable_file,
        &is_readable_file,
    );
    if json {
        println!("{}", serde_json::to_string_pretty(&manifest)?);
        return Ok(());
    }
    for (key, value) in manifest.as_object().into_iter().flatten() {
        print_entry(key, value);
    }
    Ok(())
}

/// One manifest field as a line. A nested map is either a located entry, which
/// has a resolution, or a group of them, which is flattened one level so every
/// path stays on its own line.
fn print_entry(key: &str, value: &Value) {
    match value {
        Value::Object(entry) if entry.contains_key("resolved") => {
            let path = entry
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("<unresolved>");
            let source = entry.get("source").and_then(Value::as_str).unwrap_or("-");
            println!("{key:<17} {path}  [{source}]");
        }
        Value::Object(group) => {
            for (nested, value) in group {
                print_entry(&format!("{key}.{nested}"), value);
            }
        }
        Value::String(text) => println!("{key:<17} {text}"),
        Value::Null => println!("{key:<17} <unresolved>"),
        other => println!("{key:<17} {other}"),
    }
}

/// Store and journal directories for the current project, resolved without
/// creating anything: reporting a location must not have side effects.
fn store_paths() -> StorePaths {
    let Ok(cwd) = std::env::current_dir() else {
        return StorePaths::default();
    };
    let resolved = ResolvedStore::resolve_from_process(&cwd, Engine::TokenZero, &[]);
    StorePaths::from_store_root(resolved.engine_dir().to_path_buf())
}

fn read_stdin(events: mpsc::Sender<Event>) {
    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        loop {
            let mut bytes = Vec::new();
            let read = Read::by_ref(&mut reader)
                .take((MAX_SESSION_FRAME as u64).saturating_add(2))
                .read_until(b'\n', &mut bytes);
            match read {
                Ok(0) => break,
                Ok(_) => {}
                Err(error) => {
                    let _ = events.send(Event::InputError(error.to_string()));
                    break;
                }
            }
            let ended = bytes.last() == Some(&b'\n');
            if ended {
                bytes.pop();
                if bytes.last() == Some(&b'\r') {
                    bytes.pop();
                }
            }
            if bytes.len() > MAX_SESSION_FRAME {
                if !ended && let Err(error) = drain_input_frame(&mut reader) {
                    let _ = events.send(Event::InputError(error.to_string()));
                    break;
                }
                if events
                    .send(Event::InputError(format!(
                        "input frame exceeded {MAX_SESSION_FRAME} bytes"
                    )))
                    .is_err()
                {
                    return;
                }
                continue;
            }
            let line = match String::from_utf8(bytes) {
                Ok(line) if line.trim().is_empty() => continue,
                Ok(line) => line,
                Err(error) => {
                    if events.send(Event::InputError(error.to_string())).is_err() {
                        return;
                    }
                    continue;
                }
            };
            match serde_json::from_str(&line) {
                Ok(frame) => {
                    if events.send(Event::Input(frame)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    if events.send(Event::InputError(error.to_string())).is_err() {
                        return;
                    }
                }
            }
        }
        let _ = events.send(Event::InputClosed);
    });
}

fn drain_input_frame(reader: &mut impl BufRead) -> io::Result<()> {
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(());
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(());
        }
    }
}

fn spawn_cell(
    cell_id: String,
    source: String,
    timeout_ms: Option<u64>,
    generation: u64,
    request_id: u64,
    session: Arc<AggregateSession>,
    events: mpsc::Sender<Event>,
) {
    thread::spawn(move || {
        let started = Instant::now();
        let timeout = Duration::from_millis(
            timeout_ms
                .unwrap_or(DEFAULT_EXECUTION_TIMEOUT_MS)
                .clamp(1, DEFAULT_EXECUTION_TIMEOUT_MS),
        );
        let outcome = match session.execute(generation, request_id, source, timeout) {
            Ok(result) => CellOutcome::Result(result.value),
            Err(error) => outcome_from_session_error(&error),
        };
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let _ = events.send(Event::Complete {
            cell_id,
            outcome,
            duration_ms,
        });
    });
}

/// A replaced or terminating session is a cancellation, not a user error:
/// terminate turns it into the terminated outcome, and any cell that outlived
/// its generation is dropped by the main loop.
fn outcome_from_session_error(error: &AggregateSessionError) -> CellOutcome {
    match error.code {
        AggregateSessionFailureCode::StaleGeneration | AggregateSessionFailureCode::Terminating => {
            CellOutcome::Terminated
        }
        _ => CellOutcome::Error(error.to_string()),
    }
}

fn initial_generation() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};

    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let generation = time ^ u64::from(std::process::id()).rotate_left(32);
    generation.max(1)
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
    generation: u64,
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
        if let Some(cell) = cells.get_mut(&cell_id)
            && let Some(waiter) = cell.waiter.take()
        {
            let elapsed_ms = u64::try_from(cell.started.elapsed().as_millis()).unwrap_or(u64::MAX);
            write_frame(
                writer,
                &json!({"type":"response","id":waiter.request_id,"ok":true,"kind":"yielded","cellId":cell_id,"durationMs":elapsed_ms,"generation":generation,"contentItems":[]}),
            )?;
        }
    }
    Ok(())
}
fn write_outcome(
    writer: &mut BufWriter<io::StdoutLock<'_>>,
    id: u64,
    cell_id: &str,
    outcome: CellOutcome,
    duration_ms: Option<u64>,
    generation: u64,
) -> io::Result<()> {
    let mut frame = match outcome {
        CellOutcome::Result(value) => {
            let text = match value {
                Value::String(v) => v,
                v => serde_json::to_string(&v).unwrap_or_else(|_| "null".to_owned()),
            };
            json!({"type":"response","id":id,"ok":true,"kind":"result","cellId":cell_id,"generation":generation,"contentItems":[{"type":"input_text","text":text}]})
        }
        CellOutcome::Error(message) => {
            let message = finalize_visible_error(&message);
            json!({"type":"response","id":id,"ok":true,"kind":"result","cellId":cell_id,"generation":generation,"errorText":message,"contentItems":[]})
        }
        CellOutcome::Terminated => {
            json!({"type":"response","id":id,"ok":true,"kind":"terminated","cellId":cell_id,"generation":generation,"contentItems":[]})
        }
    };
    if let (Some(map), Some(duration_ms)) = (frame.as_object_mut(), duration_ms) {
        map.insert("durationMs".to_owned(), json!(duration_ms));
    }
    write_frame(writer, &frame)
}
fn write_missing(
    writer: &mut BufWriter<io::StdoutLock<'_>>,
    id: u64,
    cell_id: &str,
    generation: u64,
) -> io::Result<()> {
    write_frame(
        writer,
        &json!({"type":"response","id":id,"ok":true,"kind":"missing","cellId":cell_id,"missingCell":true,"generation":generation,"contentItems":[]}),
    )
}
fn write_error(
    writer: &mut BufWriter<io::StdoutLock<'_>>,
    id: u64,
    generation: u64,
    message: &str,
) -> io::Result<()> {
    write_frame(
        writer,
        &json!({"type":"response","id":id,"ok":false,"generation":generation,"error":finalize_visible_error(message)}),
    )
}
fn write_frame(writer: &mut impl Write, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()
}
