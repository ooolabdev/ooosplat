//! Durable, owner-scoped fault consumption. This is test evidence, never a work order.
use super::*;
use crate::{project::PipelineStateFile, tasks::TaskRecord};
use std::{collections::BTreeMap, sync::Condvar};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Barrier {
    pub id: String,
    pub operation: String,
    pub boundary: String,
    #[serde(default)]
    pub task: Option<usize>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default = "one")]
    pub occurrence: usize,
}
fn one() -> usize {
    1
}
fn portable(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Scope {
    pub task_id: Option<Uuid>,
    pub run_id: Option<Uuid>,
    pub task_ordinal: usize,
    pub run_ordinal: usize,
    pub source_id: Option<String>,
    pub source_name: Option<String>,
    pub operation_id: Option<u64>,
}
thread_local! {
    static PROCESS_WRITER: std::cell::RefCell<Option<Scope>> = const { std::cell::RefCell::new(None) };
}
tokio::task_local! { static EXECUTION_SCOPE: Scope; }
pub async fn with_run<T>(run: Uuid, execute: impl std::future::Future<Output = T>) -> T {
    let scope = HARNESS.get().and_then(|h| {
        h.lifecycle
            .state
            .lock()
            .unwrap()
            .ledger
            .bindings
            .iter()
            .find(|binding| binding.scope.run_id == Some(run))
            .map(|binding| binding.scope.clone())
    });
    match scope {
        Some(scope) => EXECUTION_SCOPE.scope(scope, execute).await,
        None => execute.await,
    }
}
pub fn with_process_writer<T>(scope: &Scope, write: impl FnOnce() -> Result<T>) -> Result<T> {
    struct Restore(Option<Scope>);
    impl Drop for Restore {
        fn drop(&mut self) {
            PROCESS_WRITER.with(|slot| {
                slot.replace(self.0.take());
            });
        }
    }
    let _restore = Restore(PROCESS_WRITER.with(|slot| slot.replace(Some(scope.clone()))));
    write()
}
pub fn model_partial(path: &Path) -> Result<()> {
    let scope = PROCESS_WRITER.with(|slot| slot.borrow().clone());
    if let Some(scope) = scope {
        safe_path(&host()?.root, path)?;
        boundary(
            &scope,
            "model",
            "partial",
            json!({"written":1,"total":3,"path":path}),
        )?;
    }
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub id: String,
    pub rule_id: String,
    pub scope: Scope,
    pub operation: String,
    pub boundary: String,
    pub occurrence: usize,
    pub evidence: Value,
    pub disposition: String,
}
#[derive(Clone, Serialize, Deserialize)]
struct Source {
    id: String,
    name: String,
    paths: Vec<PathBuf>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Binding {
    scope: Scope,
    project: PathBuf,
    sources: Vec<Source>,
}
#[derive(Default, Serialize, Deserialize)]
struct Ledger {
    bindings: Vec<Binding>,
    counts: BTreeMap<String, usize>,
    hits: Vec<Hit>,
    consumed: BTreeSet<String>,
}
#[derive(Default)]
struct State {
    ledger: Ledger,
    released: BTreeSet<String>,
    cancelled: BTreeSet<Uuid>,
}
#[derive(Default)]
pub(crate) struct Lifecycle {
    state: Mutex<State>,
    changed: Condvar,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LateRule {
    pub id: String,
    pub task: usize,
    pub operation: String,
}
#[derive(Clone)]
pub(crate) struct Deferred {
    scope: Scope,
    observer: crate::process::ProcessObserver,
    update: crate::process::ProcessUpdate,
}
pub fn observe_process(spec: &crate::process::ProcessSpec, update: &crate::process::ProcessUpdate) {
    let Some(h) = HARNESS.get() else { return };
    let Some(rule) = &h.scenario.late_event else {
        return;
    };
    if !matches!(update, crate::process::ProcessUpdate::Resources(_)) {
        return;
    }
    let Some(observer) = spec.observer.clone() else {
        return;
    };
    let args = spec
        .args
        .iter()
        .map(|s| s.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let scope = process_scope(&args, spec.working_directory.as_deref());
    if scope.task_ordinal != rule.task
        || super::engine::operation(spec)
            .ok()
            .is_none_or(|(engine, operation)| format!("{engine}:{operation}") != rule.operation)
    {
        return;
    }
    h.deferred
        .lock()
        .unwrap()
        .entry(rule.id.clone())
        .or_insert(Deferred {
            scope,
            observer,
            update: update.clone(),
        });
}

#[tauri::command]
pub fn e2e_deliver_late(
    id: String,
    task_id: Uuid,
    run_id: Uuid,
) -> std::result::Result<(), String> {
    let deliver = || -> Result<()> {
        let h = host()?;
        let event = h
            .deferred
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or_else(|| invalid("no captured declared event"))?;
        if event.scope.task_id != Some(task_id) || event.scope.run_id != Some(run_id) {
            return Err(invalid("foreign late event"));
        }
        let mut state = h.lifecycle.state.lock().unwrap();
        if !state.ledger.consumed.insert(format!("late:{id}")) {
            return Err(invalid("late event already delivered"));
        }
        persist(h, &state.ledger)?;
        drop(state);
        (event.observer)(event.update);
        Ok(())
    };
    deliver().map_err(|e| e.to_string())
}

fn persist(h: &Harness, ledger: &Ledger) -> Result<()> {
    save_evidence(&h.root.join("lifecycle.json"), ledger)
}

pub(super) fn initialize(h: &Harness) -> Result<()> {
    let path = h.root.join("lifecycle.json");
    let mut ledger: Ledger = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ledger::default(),
        Err(e) => return Err(e.into()),
    };
    let mut ids = BTreeSet::new();
    for rule in &h.scenario.barriers {
        if rule.id.is_empty()
            || !ids.insert(&rule.id)
            || rule.occurrence == 0
            || !matches!(
                rule.boundary.as_str(),
                "partial" | "written" | "checkpoint" | "entered"
            )
        {
            return Err(invalid("invalid or duplicate lifecycle barrier"));
        }
    }
    for hit in &mut ledger.hits {
        if hit.disposition == "pending" {
            hit.disposition = "interrupted".into();
        }
    }
    persist(h, &ledger)?;
    h.lifecycle.state.lock().unwrap().ledger = ledger;
    Ok(())
}

pub fn bind_task(task: &TaskRecord) -> Result<()> {
    let Some(h) = HARNESS.get() else {
        return Ok(());
    };
    let Some(run_id) = task.run_id else {
        return Ok(());
    };
    let Some(project) = task.project_path.clone().or_else(|| {
        task.creation_intent
            .as_ref()
            .map(|i| i.project_path.clone())
    }) else {
        return Ok(());
    };
    safe_path(&h.root, &project)?;
    let mut state = h.lifecycle.state.lock().unwrap();
    if state
        .ledger
        .bindings
        .iter()
        .any(|b| b.scope.run_id == Some(run_id))
    {
        return Ok(());
    }
    let tasks = state
        .ledger
        .bindings
        .iter()
        .filter_map(|b| b.scope.task_id)
        .collect::<BTreeSet<_>>();
    let ordinal = state
        .ledger
        .bindings
        .iter()
        .find(|b| b.scope.task_id == Some(task.task_id))
        .map_or(tasks.len() + 1, |b| b.scope.task_ordinal);
    let items = task
        .request_parameters
        .as_ref()
        .and_then(|v| v.pointer("/inputs/items"))
        .and_then(Value::as_array);
    let sources = items
        .into_iter()
        .flatten()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?.to_owned();
            let source = item.get("source")?;
            let paths = if let Some(path) = source.get("path").and_then(Value::as_str) {
                vec![PathBuf::from(path)]
            } else {
                source
                    .get("paths")?
                    .as_array()?
                    .iter()
                    .filter_map(Value::as_str)
                    .map(PathBuf::from)
                    .collect()
            };
            let paths = paths.into_iter().map(|p| portable(&p)).collect::<Vec<_>>();
            let first = paths.first()?;
            let name = first
                .strip_prefix(h.root.join("fixtures"))
                .ok()?
                .to_string_lossy()
                .replace('\\', "/");
            Some(Source { id, name, paths })
        })
        .collect();
    state.ledger.bindings.push(Binding {
        project,
        sources,
        scope: Scope {
            task_id: Some(task.task_id),
            run_id: Some(run_id),
            task_ordinal: ordinal,
            run_ordinal: task.runs.len(),
            ..Scope::default()
        },
    });
    persist(h, &state.ledger)
}

pub fn scope(path: &Path, source_id: Option<&str>) -> Option<Scope> {
    let h = HARNESS.get()?;
    let path = portable(path);
    let state = h.lifecycle.state.lock().unwrap();
    let binding = state
        .ledger
        .bindings
        .iter()
        .rev()
        .find(|b| path.starts_with(&b.project))?;
    let mut scope = binding.scope.clone();
    let index = path
        .strip_prefix(&binding.project)
        .ok()?
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let batch = index
        .windows(2)
        .find(|p| p[0] == "inputs")
        .and_then(|p| p[1].parse::<usize>().ok());
    let source = source_id
        .and_then(|id| binding.sources.iter().find(|s| s.id == id || s.name == id))
        .or_else(|| batch.and_then(|n| binding.sources.get(n)));
    if let Some(source) = source {
        scope.source_id = Some(source.id.clone());
        scope.source_name = Some(source.name.clone());
    } else if let Some(id) = source_id {
        scope.source_id = Some(id.to_owned());
    }
    Some(scope)
}

pub fn process_scope(args: &[String], working: Option<&Path>) -> Scope {
    let context = EXECUTION_SCOPE.try_with(Clone::clone).ok();
    let Some(h) = HARNESS.get() else {
        return context.unwrap_or_default();
    };
    let mut found = context
        .or_else(|| working.and_then(|p| scope(p, None)))
        .or_else(|| args.iter().find_map(|a| scope(Path::new(a), None)))
        .unwrap_or_default();
    let state = h.lifecycle.state.lock().unwrap();
    if let Some(binding) = state
        .ledger
        .bindings
        .iter()
        .find(|b| b.scope.run_id == found.run_id)
    {
        if let Some(source) = binding.sources.iter().find(|s| {
            s.paths
                .iter()
                .any(|p| args.iter().any(|a| portable(Path::new(a)) == *p))
        }) {
            found.source_id = Some(source.id.clone());
            found.source_name = Some(source.name.clone());
        }
    }
    found
}

pub fn source_members(scope: &Scope) -> Vec<(String, Vec<String>)> {
    let Some(h) = HARNESS.get() else {
        return vec![];
    };
    let state = h.lifecycle.state.lock().unwrap();
    let Some(binding) = state
        .ledger
        .bindings
        .iter()
        .find(|b| b.scope.run_id == scope.run_id)
    else {
        return vec![];
    };
    let Some(saved) = std::fs::read(binding.project.join("state.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return vec![];
    };
    saved
        .pointer("/inputs/sources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|source| {
            let id = source["id"].as_str().unwrap_or_default();
            let name = binding
                .sources
                .iter()
                .find(|s| s.id == id)
                .map_or(id, |s| s.name.as_str())
                .to_owned();
            let names = source["names"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect();
            (name, names)
        })
        .collect()
}

pub fn cancel(run: Uuid) {
    if let Some(h) = HARNESS.get() {
        h.lifecycle.state.lock().unwrap().cancelled.insert(run);
        h.lifecycle.changed.notify_all();
    }
}

fn committed(h: &Harness, scope: &Scope) -> bool {
    let state = h.lifecycle.state.lock().unwrap();
    let Some(binding) = state
        .ledger
        .bindings
        .iter()
        .find(|b| b.scope.run_id == scope.run_id)
    else {
        return false;
    };
    std::fs::read(binding.project.join("work/publish-transaction.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.get("phase").and_then(Value::as_str).map(str::to_owned))
        .is_some_and(|p| {
            matches!(
                p.as_str(),
                "committed" | "projectViewsReconciled" | "reconciled"
            )
        })
}

/// A real writer calls this only after its selected bytes have been written.
pub fn boundary(scope: &Scope, operation: &str, boundary: &str, evidence: Value) -> Result<()> {
    let Some(h) = HARNESS.get() else {
        return Ok(());
    };
    boundary_in(h, scope, operation, boundary, evidence)
}
fn boundary_in(
    h: &Harness,
    scope: &Scope,
    operation: &str,
    boundary: &str,
    evidence: Value,
) -> Result<()> {
    if scope.task_id.is_none() {
        return Ok(());
    }
    let rules = h
        .scenario
        .barriers
        .iter()
        .filter(|r| {
            r.operation == operation
                && r.boundary == boundary
                && r.task.is_none_or(|n| n == scope.task_ordinal)
                && r.source.as_ref().is_none_or(|s| {
                    scope.source_name.as_ref() == Some(s) || scope.source_id.as_ref() == Some(s)
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    for rule in rules {
        let observe_only = committed(h, scope);
        let mut state = h.lifecycle.state.lock().unwrap();
        if state.ledger.consumed.contains(&rule.id) {
            continue;
        }
        let count = state.ledger.counts.entry(rule.id.clone()).or_default();
        *count += 1;
        let occurrence = *count;
        if occurrence != rule.occurrence {
            persist(h, &state.ledger)?;
            continue;
        }
        let id = format!("{}:{}", rule.id, state.ledger.hits.len() + 1);
        state.ledger.consumed.insert(rule.id.clone());
        state.ledger.hits.push(Hit {
            id: id.clone(),
            rule_id: rule.id,
            scope: scope.clone(),
            operation: operation.into(),
            boundary: boundary.into(),
            occurrence,
            evidence: evidence.clone(),
            disposition: "pending".into(),
        });
        persist(h, &state.ledger)?;
        h.lifecycle.changed.notify_all();
        loop {
            let cancelled = scope.run_id.is_some_and(|r| state.cancelled.contains(&r));
            if state.released.contains(&id) || cancelled {
                state
                    .ledger
                    .hits
                    .iter_mut()
                    .find(|hit| hit.id == id)
                    .unwrap()
                    .disposition = if cancelled { "cancelled" } else { "released" }.into();
                persist(h, &state.ledger)?;
                if cancelled && !observe_only {
                    return Err(SplatError::Cancelled);
                }
                break;
            }
            state = h.lifecycle.changed.wait(state).unwrap();
        }
    }
    Ok(())
}

pub async fn at(
    path: &Path,
    source: Option<&str>,
    operation: &str,
    boundary_name: &str,
    evidence: Value,
) -> Result<()> {
    let Some(scope) = scope(path, source) else {
        return Ok(());
    };
    safe_path(&host()?.root, path)?;
    at_scope(scope, operation, boundary_name, evidence).await
}
pub async fn at_scope(
    scope: Scope,
    operation: &str,
    boundary_name: &str,
    evidence: Value,
) -> Result<()> {
    let (operation, boundary_name) = (operation.to_owned(), boundary_name.to_owned());
    tokio::task::spawn_blocking(move || boundary(&scope, &operation, &boundary_name, evidence))
        .await
        .map_err(invalid)?
}
pub async fn run(run: Option<Uuid>, operation: &str, boundary_name: &str) -> Result<()> {
    let binding = HARNESS.get().and_then(|h| {
        h.lifecycle
            .state
            .lock()
            .unwrap()
            .ledger
            .bindings
            .iter()
            .find(|b| b.scope.run_id == run)
            .cloned()
    });
    if let Some(binding) = binding {
        at_scope(binding.scope, operation, boundary_name, json!({})).await?;
    }
    Ok(())
}

#[derive(Clone)]
pub struct Transition {
    scope: Scope,
    operation: String,
    evidence: Value,
}
pub async fn before_state(path: &Path, state: &PipelineStateFile) -> Result<Vec<Transition>> {
    let Some(scope) = scope(path, None) else {
        return Ok(vec![]);
    };
    safe_path(&host()?.root, path)?;
    let previous = std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .unwrap_or(Value::Null);
    let next = serde_json::to_value(state)?;
    let mut transitions = Vec::new();
    let mut add = |operation: &str, source: Option<&str>, evidence: Value| {
        let mut scoped = scope.clone();
        if let Some(id) = source {
            if let Some(actual) = self::scope(path, Some(id)) {
                scoped = actual;
            }
        }
        transitions.push(Transition {
            scope: scoped,
            operation: operation.into(),
            evidence,
        });
    };
    for (field, operation) in [
        ("/creation/ready", "project"),
        ("/inputs/prepared", "prepared"),
        ("/featuresComplete", "features"),
        ("/matchingComplete", "matching"),
        ("/reconstructionComplete", "reconstruction"),
        ("/brushComplete", "candidate"),
        ("/brushTraining/oomRetryUsed", "oomConsumed"),
    ] {
        if next.pointer(field) == Some(&Value::Bool(true))
            && previous.pointer(field) != Some(&Value::Bool(true))
        {
            add(operation, None, json!({"field":field}));
        }
    }
    if next.get("stage").and_then(Value::as_str) == Some("completed")
        && previous.get("stage").and_then(Value::as_str) != Some("completed")
    {
        add("projectState", None, json!({"stage":"completed"}));
    }
    for (field, operation) in [
        ("/generationPolicy", "configuration"),
        ("/inputs/referenceImage", "trainingDataset"),
        ("/validatedOutput", "candidateSummary"),
    ] {
        if next.pointer(field).is_some_and(|v| !v.is_null())
            && previous.pointer(field) != next.pointer(field)
        {
            add(operation, None, json!({"field":field}));
        }
    }
    let old_sources = previous
        .pointer("/inputs/sources")
        .and_then(Value::as_array);
    for source in next
        .pointer("/inputs/sources")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let id = source.get("id").and_then(Value::as_str).unwrap_or_default();
        if !old_sources
            .into_iter()
            .flatten()
            .any(|s| s.get("id") == source.get("id"))
        {
            add(
                "sourcePrepared",
                Some(id),
                json!({"names":source.get("names"),"frames":source.get("frames")}),
            );
        }
    }
    if next
        .pointer("/reshoot/recipe/trainingFallbacks")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        > previous
            .pointer("/reshoot/recipe/trainingFallbacks")
            .and_then(Value::as_u64)
            .unwrap_or(0)
    {
        add("fallbackConsumed", None, json!({}));
    }
    for transition in &transitions {
        at_scope(
            transition.scope.clone(),
            &transition.operation,
            "written",
            transition.evidence.clone(),
        )
        .await?;
    }
    Ok(transitions)
}
pub async fn after_state(transitions: Vec<Transition>) -> Result<()> {
    for transition in transitions {
        at_scope(
            transition.scope,
            &transition.operation,
            "checkpoint",
            transition.evidence,
        )
        .await?;
    }
    Ok(())
}
pub fn partial(path: &Path, operation: &str, written: usize, total: usize) -> Result<()> {
    if let Some(scope) = PROCESS_WRITER
        .with(|slot| slot.borrow().clone())
        .or_else(|| scope(path, None))
    {
        safe_path(&host()?.root, path)?;
        boundary(
            &scope,
            operation,
            "partial",
            json!({"written":written,"total":total,"path":path}),
        )?;
    }
    Ok(())
}
pub(super) fn snapshot(h: &Harness) -> Value {
    let state = h.lifecycle.state.lock().unwrap();
    json!({"hits":state.ledger.hits,"consumed":state.ledger.consumed,"counts":state.ledger.counts})
}
pub(super) fn release(h: &Harness, id: &str, task: Uuid, run: Uuid) -> Result<()> {
    let mut state = h.lifecycle.state.lock().unwrap();
    let hit = state
        .ledger
        .hits
        .iter()
        .find(|v| v.id == id)
        .ok_or_else(|| invalid("unknown lifecycle hit"))?;
    if hit.scope.task_id != Some(task)
        || hit.scope.run_id != Some(run)
        || hit.disposition != "pending"
    {
        return Err(invalid("stale or foreign lifecycle release"));
    }
    if !state.released.insert(id.to_owned()) {
        return Err(invalid("lifecycle hit already released"));
    }
    h.lifecycle.changed.notify_all();
    Ok(())
}

#[tauri::command]
pub fn e2e_release_hit(
    hit_id: String,
    task_id: Uuid,
    run_id: Uuid,
) -> std::result::Result<(), String> {
    release(host().map_err(|e| e.to_string())?, &hit_id, task_id, run_id).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn analysis_process_context_is_bound_to_the_actual_run_and_does_not_leak() {
        let scope = Scope {
            task_id: Some(Uuid::new_v4()),
            run_id: Some(Uuid::new_v4()),
            ..Default::default()
        };
        EXECUTION_SCOPE
            .scope(scope.clone(), async {
                let actual = process_scope(&[], None);
                assert_eq!(actual.task_id, scope.task_id);
                assert_eq!(actual.run_id, scope.run_id);
            })
            .await;
        assert!(EXECUTION_SCOPE.try_with(Clone::clone).is_err());
    }
    #[test]
    fn writer_scope_is_invocation_local_and_restored_after_failure() {
        let scope = Scope {
            task_id: Some(Uuid::new_v4()),
            run_id: Some(Uuid::new_v4()),
            source_id: Some("second-source".into()),
            operation_id: Some(9),
            ..Default::default()
        };
        let result: Result<()> = with_process_writer(&scope, || {
            PROCESS_WRITER.with(|slot| {
                assert_eq!(slot.borrow().as_ref().unwrap().source_id, scope.source_id)
            });
            Err(invalid("interrupted writer"))
        });
        assert!(result.is_err());
        PROCESS_WRITER.with(|slot| assert!(slot.borrow().is_none()));
    }
    fn harness(root: &Path) -> Harness {
        Harness {root:root.into(),scenario:serde_json::from_value(json!({"name":"ledger","inputs":[{"name":"photos","kind":"images"}],"allowedOperations":["brush:train"],"barriers":[{"id":"once","operation":"training","boundary":"partial","task":1}]})).unwrap(),calls:Mutex::default(),released:Mutex::default(),pending:Mutex::default(),changed:Notify::new(),lifecycle:Lifecycle::default(),deferred:Mutex::default()}
    }
    #[test]
    fn durable_hit_rejects_foreign_release_and_is_not_replayed_after_restart() {
        let directory = tempfile::tempdir().unwrap();
        let h = std::sync::Arc::new(harness(directory.path()));
        initialize(&h).unwrap();
        let scope = Scope {
            task_id: Some(Uuid::new_v4()),
            run_id: Some(Uuid::new_v4()),
            task_ordinal: 1,
            source_id: Some("second-video".into()),
            ..Default::default()
        };
        let worker_host = h.clone();
        let worker_scope = scope.clone();
        let worker = std::thread::spawn(move || {
            boundary_in(
                &worker_host,
                &worker_scope,
                "training",
                "partial",
                json!({"written":32,"total":64}),
            )
        });
        let mut state = h.lifecycle.state.lock().unwrap();
        while state.ledger.hits.is_empty() {
            state = h.lifecycle.changed.wait(state).unwrap();
        }
        let hit = state.ledger.hits[0].clone();
        drop(state);
        let durable: Ledger = serde_json::from_slice(
            &std::fs::read(directory.path().join("lifecycle.json")).unwrap(),
        )
        .unwrap();
        assert!(durable.consumed.contains("once"));
        assert_eq!(
            durable.hits[0].scope.source_id.as_deref(),
            Some("second-video")
        );
        assert!(release(&h, &hit.id, Uuid::new_v4(), scope.run_id.unwrap()).is_err());
        assert!(release(&h, &hit.id, scope.task_id.unwrap(), Uuid::new_v4()).is_err());
        release(&h, &hit.id, scope.task_id.unwrap(), scope.run_id.unwrap()).unwrap();
        assert!(release(&h, &hit.id, scope.task_id.unwrap(), scope.run_id.unwrap()).is_err());
        worker.join().unwrap().unwrap();
        let restarted = harness(directory.path());
        initialize(&restarted).unwrap();
        let mut resumed = scope;
        resumed.run_id = Some(Uuid::new_v4());
        resumed.run_ordinal = 2;
        boundary_in(
            &restarted,
            &resumed,
            "training",
            "partial",
            json!({"written":32,"total":64}),
        )
        .unwrap();
        assert_eq!(
            restarted.lifecycle.state.lock().unwrap().ledger.hits.len(),
            1
        );
    }
    #[test]
    fn ledger_round_trip_retains_scope_and_consumption() {
        let run = Uuid::new_v4();
        let task = Uuid::new_v4();
        let mut ledger = Ledger::default();
        ledger.consumed.insert("once".into());
        ledger.hits.push(Hit {
            id: "once:1".into(),
            rule_id: "once".into(),
            scope: Scope {
                task_id: Some(task),
                run_id: Some(run),
                source_id: Some("video-b".into()),
                ..Default::default()
            },
            operation: "extract".into(),
            boundary: "partial".into(),
            occurrence: 1,
            evidence: json!({"written":1}),
            disposition: "pending".into(),
        });
        let restored: Ledger =
            serde_json::from_slice(&serde_json::to_vec(&ledger).unwrap()).unwrap();
        assert!(restored.consumed.contains("once"));
        assert_eq!(restored.hits[0].scope.run_id, Some(run));
        assert_eq!(restored.hits[0].scope.task_id, Some(task));
    }
}
