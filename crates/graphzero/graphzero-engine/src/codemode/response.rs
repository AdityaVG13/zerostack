//! Response assembly, ref materialization, and durable execution-file writes.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use super::errors::{policy_error, substrate_error};
use super::types::{
    BindingResult, CodeModeError, CodeModeResponse, CodeModeTelemetry, MAX_OUTPUT_BYTES,
    MAX_RESULT_REF_BYTES, StepRecord,
};
use super::utils::{
    compact_query_alias, safe_execution_path_component, store_blob_ref, store_query_json_ref,
    store_query_ref,
};
use graphzero_store::store::query::tokens_for_str;

pub(crate) fn finish_response(
    store_root: &Path,
    execution_id: String,
    kind: &str,
    code: &str,
    steps: Vec<StepRecord>,
    mut telemetry: CodeModeTelemetry,
    mut result: Result<BindingResult, CodeModeError>,
) -> CodeModeResponse {
    let serialized_result = match result
        .as_ref()
        .ok()
        .map(|value| serialize_result_value(&value.value))
        .transpose()
    {
        Ok(value) => value,
        Err(error) => {
            result = Err(error);
            None
        }
    };
    let ack = if result.is_ok() { "C" } else { "X0" };
    telemetry.visible_ack = ack.to_string();
    telemetry.status = if result.is_ok() { "ok" } else { "error" }.to_string();

    let (result_value, error_value) = match result {
        Ok(v) => (Some(v), None),
        Err(e) => (None, Some(e)),
    };
    let response_error = error_value.clone();

    let mut telemetry_refs = telemetry.refs.clone();
    if let Some(v) = &result_value {
        for r in &v.refs {
            if !telemetry_refs.contains(r) {
                telemetry_refs.push(r.clone());
            }
        }
        telemetry.bytes_materialized = telemetry
            .bytes_materialized
            .saturating_add(v.bytes_materialized);
    }

    let safe_execution_id = safe_execution_path_component(&execution_id);
    let codemode_ref = |part: &str| format!("gz://codemode/execution/{safe_execution_id}/{part}");
    let execution_ref = format!("gz://codemode/execution/{safe_execution_id}");
    let steps_ref = codemode_ref("steps");
    let telemetry_ref = codemode_ref("telemetry");
    let result_ref = result_value
        .as_ref()
        .and_then(|value| value.value.as_str())
        .filter(|reference| graphzero_store::GzRef::parse(reference).is_ok())
        .map(str::to_owned)
        .or_else(|| serialized_result.as_ref().map(|_| codemode_ref("result")));
    let error_ref = error_value.as_ref().map(|_| codemode_ref("error"));

    // Durable gz:// recovery refs must be visible to hub extractTypedRefs /
    // handoff even when structuredContent is minimized.
    for durable in [&execution_ref, &steps_ref, &telemetry_ref] {
        if !telemetry_refs.contains(durable) {
            telemetry_refs.push(durable.clone());
        }
    }
    if let Some(r) = &result_ref
        && !telemetry_refs.contains(r)
    {
        telemetry_refs.push(r.clone());
    }
    if let Some(e) = &error_ref
        && !telemetry_refs.contains(e)
    {
        telemetry_refs.push(e.clone());
    }
    telemetry.refs = telemetry_refs;

    let error_json = error_value.as_ref().map(|e| json!(e));
    let result_tokens = serialized_result
        .as_deref()
        .map(tokens_for_str)
        .unwrap_or(0);
    let error_tokens = error_json
        .as_ref()
        .and_then(|value| serde_json::to_string(value).ok())
        .as_deref()
        .map(tokens_for_str)
        .unwrap_or(0);
    telemetry.raw_token_estimate = result_tokens
        .max(error_tokens)
        .max(telemetry.bytes_materialized.div_ceil(4))
        .max(1);
    telemetry.visible_token_estimate = tokens_for_str(ack).max(1);
    telemetry.measurement_coverage_pct = 100;
    let steps_json = json!(steps);
    let telemetry_json = json!(telemetry);

    let persist_outcome = persist_execution_artifacts(
        store_root,
        &execution_id,
        kind,
        code,
        &steps_json,
        &telemetry_json,
        serialized_result.as_deref(),
        error_json.as_ref(),
        ack,
        &steps_ref,
        &telemetry_ref,
        &result_ref,
        &error_ref,
        &execution_ref,
        &telemetry,
    );

    let (
        ack,
        telemetry,
        response_error,
        execution_ref,
        envelope_ref,
        result_ref,
        telemetry_ref,
        steps_ref,
        error_ref,
        inline_result,
        execution_obj,
    ) = match persist_outcome {
        Ok(ok) => {
            let inline_result = serialized_result
                .as_ref()
                .filter(|json_text| json_text.len() <= MAX_OUTPUT_BYTES)
                .and_then(|_| result_value.as_ref().map(|value| value.value.clone()));
            (
                ack.to_string(),
                telemetry,
                response_error,
                execution_ref,
                ok.envelope_ref,
                result_ref,
                telemetry_ref,
                steps_ref,
                error_ref,
                inline_result,
                ok.execution_obj,
            )
        }
        Err(persist_err) => {
            // Fail closed: never return ack=C (or ghost refs) when durable bytes
            // were not published. Prefer the original domain error when present.
            let err = response_error.unwrap_or(persist_err);
            let mut telemetry = telemetry;
            telemetry.visible_ack = "X0".into();
            telemetry.status = "error".into();
            telemetry.refs.clear();
            (
                "X0".into(),
                telemetry,
                Some(err),
                String::new(),
                String::new(),
                None,
                String::new(),
                String::new(),
                None,
                None,
                json!({}),
            )
        }
    };

    let mut response = CodeModeResponse {
        ack: ack.clone(),
        execution_id,
        execution_ref,
        envelope_ref,
        result_ref,
        telemetry_ref,
        steps_ref,
        error_ref,
        visible: ack,
        telemetry,
        error: response_error,
        result: inline_result,
    };
    // Durable usage telemetry (opt-in) is separate from in-session telemetry_ref.
    let config = graphzero_store::load_telemetry_config(store_root);
    let enabled = graphzero_store::usage_telemetry_enabled(config);
    let spent = tokens_for_str(&response.compact_line()).max(1);
    response.telemetry.visible_token_estimate = spent;
    let final_telemetry = json!(response.telemetry);
    let _ = write_json_file(
        &store_root
            .join("codemode")
            .join(&safe_execution_id)
            .join("telemetry"),
        &final_telemetry,
    );
    let _ = write_json_file(
        &store_root.join("codemode").join("telemetry"),
        &final_telemetry,
    );
    let raw_mass =
        tokens_for_str(&serde_json::to_string(&execution_obj).unwrap_or_default()).max(spent);
    graphzero_store::record_codemode_accounting(store_root, enabled, raw_mass, spent);
    response
}

struct PersistOk {
    envelope_ref: String,
    execution_obj: Value,
}

fn persist_execution_artifacts(
    store_root: &Path,
    execution_id: &str,
    kind: &str,
    code: &str,
    steps_json: &Value,
    telemetry_json: &Value,
    serialized_result: Option<&str>,
    error_json: Option<&Value>,
    ack: &str,
    steps_ref: &str,
    telemetry_ref: &str,
    result_ref: &Option<String>,
    error_ref: &Option<String>,
    execution_ref: &str,
    telemetry: &CodeModeTelemetry,
) -> Result<PersistOk, CodeModeError> {
    let code_blob_ref = store_blob_ref(store_root, code.as_bytes()).map_err(|error| {
        substrate_error(
            format!("durable code blob persist failed: {error}"),
            "persist",
        )
    })?;
    let stored_steps_ref = store_query_ref(store_root, steps_json).map_err(|error| {
        substrate_error(format!("durable steps spill failed: {error}"), "persist")
    })?;
    let stored_telemetry_ref = store_query_ref(store_root, telemetry_json).map_err(|error| {
        substrate_error(
            format!("durable telemetry spill failed: {error}"),
            "persist",
        )
    })?;
    let stored_result_ref =
        match serialized_result {
            Some(json_text) => Some(store_query_json_ref(store_root, json_text).map_err(
                |error| substrate_error(format!("durable result spill failed: {error}"), "persist"),
            )?),
            None => None,
        };
    let stored_error_ref = match error_json {
        Some(v) => Some(store_query_ref(store_root, v).map_err(|error| {
            substrate_error(format!("durable error spill failed: {error}"), "persist")
        })?),
        None => None,
    };

    let mut execution_obj = json!({
        "execution_id": execution_id,
        "ns": "gz",
        "kind": kind,
        "status": telemetry.status,
        "visible_ack": ack,
        "code_ref": code_blob_ref,
        "steps_ref": steps_ref,
        "telemetry_ref": telemetry_ref,
        "result_ref": result_ref,
        "error_ref": error_ref,
        "execution_ref": execution_ref,
        "stored": {
            "steps": stored_steps_ref,
            "telemetry": stored_telemetry_ref,
            "result": stored_result_ref,
            "error": stored_error_ref,
        },
        "refs": telemetry.refs,
        "telemetry": telemetry,
    });

    let execution_store_ref = store_query_ref(store_root, &execution_obj).map_err(|error| {
        substrate_error(
            format!("durable execution spill failed: {error}"),
            "persist",
        )
    })?;
    let envelope_ref = compact_query_alias(&execution_store_ref);
    if let Some(obj) = execution_obj.as_object_mut() {
        obj.insert("envelope_ref".into(), json!(envelope_ref));
    }

    write_execution_files(
        store_root,
        execution_id,
        &ExecutionArtifacts {
            code,
            steps: steps_json,
            telemetry: telemetry_json,
            result: serialized_result,
            error: error_json,
            execution: &execution_obj,
        },
    )?;

    Ok(PersistOk {
        envelope_ref,
        execution_obj,
    })
}

fn serialize_result_value(value: &Value) -> Result<String, CodeModeError> {
    let json_text = serde_json::to_string(value).map_err(|error| {
        substrate_error(format!("result serialization failed: {error}"), "result")
    })?;
    if json_text.len() > MAX_RESULT_REF_BYTES {
        return Err(policy_error(
            format!(
                "result ref byte limit exceeded: {} > {}",
                json_text.len(),
                MAX_RESULT_REF_BYTES
            ),
            "result",
        ));
    }
    Ok(json_text)
}

pub(crate) struct ExecutionArtifacts<'a> {
    pub code: &'a str,
    pub steps: &'a Value,
    pub telemetry: &'a Value,
    pub result: Option<&'a str>,
    pub error: Option<&'a Value>,
    pub execution: &'a Value,
}

pub(crate) fn write_execution_files(
    store_root: &Path,
    execution_id: &str,
    artifacts: &ExecutionArtifacts,
) -> Result<(), CodeModeError> {
    let ExecutionArtifacts {
        code,
        steps,
        telemetry,
        result,
        error,
        execution,
    } = *artifacts;
    let safe_id = safe_execution_path_component(execution_id);
    let dir = store_root.join("codemode").join("execution").join(&safe_id);
    fs::create_dir_all(&dir).map_err(|error| {
        substrate_error(
            format!("create artifact dir {}: {error}", dir.display()),
            "persist",
        )
    })?;
    write_bytes_file(&dir.join("code"), code.as_bytes())?;
    write_json_file(&dir.join("steps"), steps)?;
    write_json_file(&dir.join("telemetry"), telemetry)?;
    if let Some(r) = result {
        write_bytes_file(&dir.join("result"), r.as_bytes())?;
    }
    if let Some(e) = error {
        write_json_file(&dir.join("error"), e)?;
    }
    write_json_file(&dir.join("execution"), execution)?;

    let base_ref = format!("gz://codemode/execution/{safe_id}");
    record_artifact_ref(&base_ref, store_root)?;
    record_artifact_ref(&format!("{base_ref}/code"), store_root)?;
    record_artifact_ref(&format!("{base_ref}/steps"), store_root)?;
    record_artifact_ref(&format!("{base_ref}/telemetry"), store_root)?;
    if result.is_some() {
        record_artifact_ref(&format!("{base_ref}/result"), store_root)?;
    }
    if error.is_some() {
        record_artifact_ref(&format!("{base_ref}/error"), store_root)?;
    }

    let latest = store_root.join("codemode");
    fs::create_dir_all(&latest).map_err(|error| {
        substrate_error(
            format!("create latest dir {}: {error}", latest.display()),
            "persist",
        )
    })?;
    write_json_file(&latest.join("steps"), steps)?;
    write_json_file(&latest.join("telemetry"), telemetry)?;
    if let Some(r) = result {
        write_bytes_file(&latest.join("result"), r.as_bytes())?;
    }
    if let Some(e) = error {
        write_json_file(&latest.join("error"), e)?;
    }
    if let Err(error) = graphzero_store::store::indexer::prune_transient_artifacts(store_root) {
        eprintln!("graphzero codemode: retention prune failed: {error:#}");
    }
    Ok(())
}

fn write_bytes_file(path: &Path, bytes: &[u8]) -> Result<(), CodeModeError> {
    graphzero_store::store::atomic_write_file(path, bytes).map_err(|error| {
        substrate_error(
            format!("write artifact {}: {error}", path.display()),
            "persist",
        )
    })
}

fn record_artifact_ref(reference: &str, store_root: &Path) -> Result<(), CodeModeError> {
    graphzero_store::store::ref_index::record_ref(reference, store_root).map_err(|error| {
        substrate_error(
            format!("record artifact ref {reference}: {error:#}"),
            "persist",
        )
    })
}

pub(crate) fn write_json_file(path: &Path, value: &Value) -> Result<(), CodeModeError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        substrate_error(
            format!("serialize artifact {}: {error}", path.display()),
            "persist",
        )
    })?;
    write_bytes_file(path, &bytes)
}
