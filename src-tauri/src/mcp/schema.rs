use serde_json::{json, Value};
pub(super) fn output(name: &str) -> Value {
    let id = json!({"type":"string","format":"uuid"});
    let nullable_id = json!({"type":["string","null"],"format":"uuid"});
    let nullable_number = json!({"type":["number","null"]});
    let status = json!({"type":"string","enum":["created","starting","running","cancelling","completed","failed","cancelled","interrupted"]});
    let error = json!({"type":["object","null"],"properties":{"code":{"type":"string"},"message":{"type":"string"},"failed_stage":{"type":["string","null"]},"engine":{"type":["string","null"]},"exit_code":{"type":["integer","null"]},"classification":{"type":["string","null"]},"classification_is_heuristic":{"type":"boolean"},"failure_id":nullable_id,"log_sources":{"type":"array","items":{"type":"string"}}}});
    let mut task = json!({"type":"object","required":["task_id","run_id","project_id","status","stage","revision","source","quality","input_path","error","result"],"properties":{
        "task_id":id,"run_id":nullable_id,"project_id":nullable_id,"project_path":{"type":["string","null"]},"input_path":{"type":"string"},"projects_root":{"type":"string"},"quality":{"type":"string","enum":["fast","balanced","high"]},"source":{"type":["string","null"],"enum":["gui","mcp",null]},"configuration_inferred":{"type":"boolean"},"project_deleted":{"type":"boolean"},"input_type":{"type":"string","enum":["video","images"]},"task_kind":{"type":"string"},"status":status,"stage":{"type":["string","null"]},"revision":{"type":"integer","minimum":1},"sequence":{"type":"integer","minimum":0},"created_at":{"type":"string","format":"date-time"},"updated_at":{"type":"string","format":"date-time"},"elapsed_ms":{"type":"integer","minimum":0},"planner_enabled":{"type":"boolean"},"configuration":{"type":"object"},"actual_configuration":{"type":["object","null"]},"progress":{"type":["number","null"],"description":"Last observed actual stage percentage, never an estimated overall percentage."},"estimated_progress":{"type":["number","null"],"description":"Estimated overall percentage from existing stage mapping."},"current":{"type":["integer","null"]},"total":{"type":["integer","null"]},"unit":{"type":["string","null"]},"eta_seconds":nullable_number,"error":error,"result":{"type":["object","null"],"properties":{"final_ply":{"type":"string"},"file_size":{"type":"integer"},"splat_count":{"type":"integer"}}},"runs":{"type":"array","items":{"type":"object","properties":{"run_id":id,"kind":{"type":"string"},"started_at":{"type":"string"},"ended_at":{"type":["string","null"]},"log_sources":{"type":"array","items":{"type":"string"}}}}}
    }});
    task["properties"]["generation_policy"] = json!({"type":["object","null"]});
    task["properties"]["automatically_retried"] = json!({"type":"boolean"});
    task["properties"]["substage"] = json!({"type":["string","null"]});
    task["properties"]["operation"] = json!({"type":["object","null"]});
    task["properties"]["runtime"] = json!({"type":["object","null"]});
    task["properties"]["error"]["properties"]["classification_version"] =
        json!({"type":["integer","null"]});
    task["properties"]["error"]["properties"]["termination"] = json!({"type":["object","null"]});
    task["properties"]["runs"]["items"]["properties"]["brush_executions_truncated"] =
        json!({"type":"boolean"});
    task["properties"]["runs"]["items"]["properties"]["continued_from_status"] =
        json!({"type":["string","null"],"enum":["failed","cancelled","interrupted",null]});
    task["properties"]["runs"]["items"]["properties"]["brush_executions"] = json!({"type":"array","maxItems":8,"items":{"type":"object","properties":{
        "operation":{"type":["object","null"]},"started_at":{"type":"string"},"ended_at":{"type":["string","null"]},"exit_code":{"type":["integer","null"]},
        "backend":{"type":["string","null"]},"device":{"type":["object","null"],"properties":{"name":{"type":"string"},"total_memory_mb":{"type":["integer","null"]},"driver_version":{"type":["string","null"]}}},"configuration":{"type":["object","null"]}
    }}});
    let mut success = match name {
        "inspect_generation_input" => {
            json!({"type":"object","required":["preflight_id","input_source_identity","input","plan","questions","gps"],"properties":{"preflight_id":id,"input_source_identity":{"type":"string"},"input":{"type":"object"},"source":{"type":["object","null"]},"generation_policy":{"type":"object"},"plan":{"type":"object"},"plan_is_estimate":{"const":true},"max_splats":{"type":"null"},"questions":{"type":"array","items":{"type":"object"}},"gps":{"type":"object"}}})
        }
        "start_model_export"
        | "get_model_export_status"
        | "confirm_model_export"
        | "cancel_model_export" => {
            json!({"type":"object","required":["export_id","project_id","format","phase","progress","confirmation","result","error"],"properties":{"export_id":id,"project_id":id,"format":{"type":"string","enum":["ply","compressed_ply","spz","sog"]},"phase":{"type":"string","enum":["starting","preparing","awaiting_confirmation","encoding","validating","publishing","cancelling","completed","failed","cancelled","interrupted"]},"progress":{"type":["number","null"],"minimum":0,"maximum":100},"edit_revision":{"type":"integer"},"confirmation":{"type":["object","null"]},"result":{"type":["object","null"]},"error":{"type":["object","null"]}}})
        }
        "inspect_reshoot_input" => {
            json!({"type":"object","required":["source","input"],"properties":{"source":{"type":"object"},"input":{"type":"object"}}})
        }
        "create_generation_task" | "get_task_status" | "cancel_task" => task,
        "start_task" | "resume_task" => {
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
    };
    if name == "get_app_status" {
        success["properties"]["generation_policy_version"] = json!({"type":"integer","minimum":1});
        success["properties"]["model_export"] = json!({"type":"object","required":["executor_ready","formats","active_export"],"properties":{
            "executor_ready":{"type":"boolean"},"busy":{"type":"boolean"},"formats":{"type":"array","items":{"enum":["ply","compressed_ply","spz","sog"]}},"active_export":{"type":["object","null"]}
        }});
        success["properties"]["engines"]["items"]["properties"]["cpu_only"] =
            json!({"type":"boolean"});
        success["properties"]["engines"]["items"]["properties"]["detail"] =
            json!({"type":"string"});
    }
    // MCP output schemas describe structured errors and pending questions too.
    let failure = json!({"type":"object","required":["error"],"properties":{
        "error":{"type":"object","required":["code","message"],"properties":{
            "code":{"type":"string"},"message":{"type":"string"},"reason":{"type":"string","enum":["colmap_disabled","unsupported_input"]},"details":{"type":["object","null"]}
        }}
    }});
    json!({"type":"object","anyOf":[success,failure]})
}
