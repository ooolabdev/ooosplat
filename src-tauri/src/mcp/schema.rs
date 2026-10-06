use serde_json::{json, Value};
pub(super) fn output(name: &str) -> Value {
    let id = json!({"type":"string","format":"uuid"});
    let nullable_id = json!({"type":["string","null"],"format":"uuid"});
    let nullable_number = json!({"type":["number","null"]});
    let status = json!({"type":"string","enum":["created","starting","running","cancelling","completed","failed","cancelled","interrupted"]});
    let error = json!({"type":["object","null"],"properties":{"code":{"type":"string"},"message":{"type":"string"},"failed_stage":{"type":["string","null"]},"engine":{"type":["string","null"]},"exit_code":{"type":["integer","null"]},"classification":{"type":["string","null"]},"classification_is_heuristic":{"type":"boolean"},"failure_id":nullable_id,"log_sources":{"type":"array","items":{"type":"string"}}}});
    let task = json!({"type":"object","required":["task_id","run_id","project_id","status","stage","revision","source","quality","input_path","error","result"],"properties":{
        "task_id":id,"run_id":nullable_id,"project_id":nullable_id,"project_path":{"type":["string","null"]},"input_path":{"type":"string"},"projects_root":{"type":"string"},"quality":{"type":"string","enum":["fast","balanced","high"]},"source":{"type":["string","null"],"enum":["gui","mcp",null]},"configuration_inferred":{"type":"boolean"},"project_deleted":{"type":"boolean"},"input_type":{"type":"string","enum":["video","images"]},"task_kind":{"type":"string"},"status":status,"stage":{"type":["string","null"]},"revision":{"type":"integer","minimum":1},"sequence":{"type":"integer","minimum":0},"created_at":{"type":"string","format":"date-time"},"updated_at":{"type":"string","format":"date-time"},"elapsed_ms":{"type":"integer","minimum":0},"planner_enabled":{"type":"boolean"},"configuration":{"type":"object"},"actual_configuration":{"type":["object","null"]},"progress":{"type":["number","null"],"description":"Last observed actual stage percentage, never an estimated overall percentage."},"estimated_progress":{"type":["number","null"],"description":"Estimated overall percentage from existing stage mapping."},"current":{"type":["integer","null"]},"total":{"type":["integer","null"]},"unit":{"type":["string","null"]},"eta_seconds":nullable_number,"error":error,"result":{"type":["object","null"],"properties":{"final_ply":{"type":"string"},"file_size":{"type":"integer"},"splat_count":{"type":"integer"}}},"runs":{"type":"array","items":{"type":"object","properties":{"run_id":id,"kind":{"type":"string"},"started_at":{"type":"string"},"ended_at":{"type":["string","null"]},"log_sources":{"type":"array","items":{"type":"string"}}}}}
    }});
    match name {
        "create_generation_task" | "get_task_status" | "cancel_task" => task,
        "start_task" => {
            json!({"type":"object","required":["accepted","task_id","run_id","status","revision"],"properties":{"accepted":{"type":"boolean"},"task_id":id,"run_id":nullable_id,"status":status,"revision":{"type":"integer"}}})
        }
        "list_tasks" => {
            json!({"type":"object","required":["tasks","next_cursor","has_more"],"properties":{"tasks":{"type":"array","items":task},"next_cursor":nullable_id,"has_more":{"type":"boolean"}}})
        }
        "get_app_status" => {
            json!({"type":"object","required":["app_version","engines","capabilities","running_task","can_start_task","recommended_poll_seconds"],"properties":{"app_version":{"type":"string"},"engines":{"type":"array","items":{"type":"object","properties":{"name":{"type":"string"},"version":{"type":["string","null"]},"available":{"type":"boolean"}}}},"capabilities":{"type":"array","items":{"type":"string"}},"authorized_input_roots":{"type":"array","items":{"type":"string"}},"running_task":{"anyOf":[task,{"type":"null"}]},"can_start_task":{"type":"boolean"},"recommended_poll_seconds":{"type":"integer"}}})
        }
        "read_task_logs" => {
            json!({"type":"object","required":["task_id","run_id","entries","next_cursor","has_more","truncated","cursor_reset","reset_reason","available_sources","content_is_untrusted"],"properties":{"task_id":id,"run_id":nullable_id,"entries":{"type":"array","items":{"type":"object","required":["source","text","partial_line"],"properties":{"source":{"type":"string"},"text":{"type":"string"},"partial_line":{"type":"boolean"}}}},"next_cursor":{"type":["string","null"]},"has_more":{"type":"boolean"},"truncated":{"type":"boolean"},"cursor_reset":{"type":"boolean"},"reset_reason":{"type":["string","null"]},"available_sources":{"type":"array","items":{"type":"string"}},"content_is_untrusted":{"type":"boolean"}}})
        }
        _ => json!({"type":"object"}),
    }
}
