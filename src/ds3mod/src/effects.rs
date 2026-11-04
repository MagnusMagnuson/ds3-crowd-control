use serde_json::Value;

use crate::game;

#[derive(Clone, Debug)]
pub enum Outcome {
    Success(String),
    Failed(String),
    Unavailable(String),
    #[allow(dead_code)]
    Retry(String),
}

pub fn schemas() -> Value {
    game::schemas_json()
}

pub fn dispatch(handler: &str, args: &Value) -> Outcome {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| execute(handler, args)));
    match result {
        Ok(outcome) => outcome,
        Err(_) => Outcome::Failed(format!("{handler} panicked and was stopped")),
    }
}

fn execute(handler: &str, args: &Value) -> Outcome {
    if let Err(reason) = game::gate() {
        return Outcome::Unavailable(reason);
    }
    if handler == "spawn_npc_invader" {
        return spawn_unavailable();
    }

    let result = match handler {
        "adjust_souls" => {
            let delta = args.get("delta").and_then(Value::as_i64).unwrap_or(-100);
            let delta = delta.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            game::adjust_souls(delta)
        }
        "use_selected_item" => game::use_selected_item(),
        "kill_player" => game::kill_player(),
        "fake_death" => game::start_fake_death(),
        "show_you_died" => game::show_you_died(),
        "empty_stamina" => game::empty_stamina(),
        "no_stamina" => {
            let seconds = args.get("seconds").and_then(Value::as_i64).unwrap_or(5);
            game::no_stamina(seconds.clamp(1, 60) as i32)
        }
        "decrease_speed" => {
            let (percent, seconds, movement_only) = speed_args(args);
            game::change_speed(-percent.clamp(1, 90), seconds, movement_only)
        }
        "increase_speed" => {
            let (percent, seconds, movement_only) = speed_args(args);
            game::change_speed(percent.clamp(1, 300), seconds, movement_only)
        }
        "one_hp" => game::one_hp(),
        "one_hit_run" => {
            let seconds = args.get("seconds").and_then(Value::as_i64).unwrap_or(30);
            game::one_hit_run(seconds.clamp(1, 300) as i32)
        }
        "set_health_percent" => {
            let percent = args.get("percent").and_then(Value::as_i64).unwrap_or(50);
            game::set_health_percent(percent.clamp(1, 100) as i32)
        }
        "set_health_random_percent" => game::set_health_random_percent(),
        "dump_player_data" => {
            let soul_count = args.get("soulCount").and_then(Value::as_i64).unwrap_or(0);
            game::dump_player_data(soul_count)
        }
        "watch_item_button" => game::start_button_watch(),
        _ => return Outcome::Unavailable(format!("unknown handler {handler}")),
    };
    match result {
        Ok(message) => Outcome::Success(message),
        Err(message) => Outcome::Failed(message),
    }
}

fn speed_args(args: &Value) -> (i32, i32, bool) {
    let percent = args.get("percent").and_then(Value::as_i64).unwrap_or(30);
    let seconds = args.get("seconds").and_then(Value::as_i64).unwrap_or(30);
    let movement_only = args.get("movementOnly").and_then(Value::as_i64).unwrap_or(0) != 0;
    (percent.clamp(1, 300) as i32, seconds.clamp(1, 300) as i32, movement_only)
}

pub fn spawn_unavailable() -> Outcome {
    Outcome::Unavailable("spawn_npc_invader is registered and not implemented yet".to_string())
}

#[cfg(test)]
pub fn gate_snapshot(snapshot: &game::Snapshot) -> Result<(), String> {
    if !snapshot.exe_match {
        return Err("this Dark Souls III build is not supported".to_string());
    }
    if !snapshot.session_known {
        return Err("the session type is unknown, so effects stay off".to_string());
    }
    if !snapshot.offline {
        return Err("effects run only while the game is in an offline session".to_string());
    }
    if !snapshot.in_game {
        return Err("there is no local player yet".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_handler_is_unavailable() {
        match spawn_unavailable() {
            Outcome::Unavailable(message) => assert!(message.contains("not implemented")),
            other => panic!("expected unavailable, got {other:?}"),
        }
    }

    #[test]
    fn spawn_is_unavailable_once_the_session_is_safe() {
        let snapshot = crate::game::Snapshot {
            exe_match: true,
            session_known: true,
            offline: true,
            in_game: true,
            character_name: "Ash".to_string(),
        };
        assert!(gate_snapshot(&snapshot).is_ok());
        match dispatch("spawn_npc_invader", &serde_json::json!({})) {
            Outcome::Unavailable(message) => {
                assert!(message.contains("not implemented") || message.contains("offline") || message.contains("not supported") || message.contains("unknown") || message.contains("no local player"));
            }
            other => panic!("expected unavailable, got {other:?}"),
        }
    }

    #[test]
    fn online_snapshot_is_rejected() {
        let snapshot = crate::game::Snapshot {
            exe_match: true,
            session_known: true,
            offline: false,
            in_game: true,
            character_name: "Ash".to_string(),
        };
        let error = gate_snapshot(&snapshot).unwrap_err();
        assert!(error.contains("offline"));
    }
}
