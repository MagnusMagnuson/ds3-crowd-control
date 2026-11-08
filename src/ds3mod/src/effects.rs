use serde_json::Value;

use crate::game;

#[derive(Clone, Debug)]
pub enum Outcome {
    Success(String),
    Failed(String),
    Unavailable(String),
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
    if let Some(reason) = game::not_ready_reason() {
        return Outcome::Retry(reason);
    }
    if let Err(reason) = game::gate() {
        return Outcome::Unavailable(reason);
    }

    let result = match handler {
        "adjust_souls" => {
            let delta = args.get("delta").and_then(Value::as_i64).unwrap_or(-100);
            let delta = delta.clamp(i32::MIN as i64, i32::MAX as i64) as i32;
            let scale = args.get("scaleWithLevel").and_then(Value::as_i64).unwrap_or(0) != 0;
            game::adjust_souls(delta, scale)
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
        "invincible" => {
            let seconds = args.get("seconds").and_then(Value::as_i64).unwrap_or(30);
            game::invincible(seconds.clamp(1, 300) as i32)
        }
        "random_weapon" => {
            let number = |name: &str, default: i64| {
                args.get(name).and_then(Value::as_i64).unwrap_or(default)
            };
            game::random_weapon(game::WeaponOptions {
                level: game::WeaponTrait::from_index(number("weaponLevel", 0)),
                infusion: game::WeaponTrait::from_index(number("infusion", 0)),
                usable_only: number("usableOnly", 1) != 0,
                remove_old: number("removeOld", 1) != 0,
                include_missing_dlc: number("includeMissingDlc", 0) != 0,
            })
        }
        "random_armor" => game::random_armor(game::ArmorOptions {
            remove_old: args.get("removeOld").and_then(Value::as_i64).unwrap_or(1) != 0,
            include_missing_dlc: args.get("includeMissingDlc").and_then(Value::as_i64).unwrap_or(0) != 0,
            style: game::ArmorStyle::from_index(
                args.get("armorStyle").and_then(Value::as_i64).unwrap_or(0),
            ),
        }),
        "temporary_armor" => {
            let number = |name: &str, default: i64| {
                args.get(name).and_then(Value::as_i64).unwrap_or(default)
            };
            game::temporary_armor(
                number("seconds", 60).clamp(1, 600) as i32,
                game::ArmorStyle::from_index(number("armorStyle", 0)),
                number("includeMissingDlc", 0) != 0,
            )
        }
        "bombardier" => {
            let seconds = args.get("seconds").and_then(Value::as_i64).unwrap_or(60);
            game::bombardier(seconds.clamp(5, 600) as i32)
        }
        "shuffle_stats" => {
            let seconds = args.get("seconds").and_then(Value::as_i64).unwrap_or(0);
            game::shuffle_stats(seconds.clamp(0, 600) as i32)
        }
        "set_health_percent" => {
            let percent = args.get("percent").and_then(Value::as_i64).unwrap_or(50);
            game::set_health_percent(percent.clamp(1, 100) as i32)
        }
        "set_health_random_percent" => game::set_health_random_percent(),
        "oversaturate" => {
            let seconds = args.get("seconds").and_then(Value::as_i64).unwrap_or(15);
            let percent = args.get("percent").and_then(Value::as_i64).unwrap_or(2000);
            game::oversaturate(seconds.clamp(1, 300) as i32, percent.clamp(100, 2000) as i32)
        }
        "blind" => {
            let seconds = args.get("seconds").and_then(Value::as_i64).unwrap_or(5);
            game::blind(seconds.clamp(1, 60) as i32)
        }
        "freeze" => {
            let seconds = args.get("seconds").and_then(Value::as_i64).unwrap_or(5);
            game::freeze(seconds.clamp(1, 60) as i32)
        }
        "invert_controls" => {
            let seconds = args.get("seconds").and_then(Value::as_i64).unwrap_or(15);
            let camera = args.get("camera").and_then(Value::as_i64).unwrap_or(1) != 0;
            crate::input::invert_controls(seconds.clamp(1, 300) as i32, camera)
        }
        "spawn_random_enemy" => {
            let amount = args.get("amount").and_then(Value::as_i64).unwrap_or(1);
            let whole_game = args.get("wholeGame").and_then(Value::as_i64).unwrap_or(0) != 0;
            let bosses = args.get("bosses").and_then(Value::as_i64).unwrap_or(1) != 0;
            let only_bosses = args.get("onlyBosses").and_then(Value::as_i64).unwrap_or(0) != 0;
            let replace_oldest = args.get("replaceOldest").and_then(Value::as_i64).unwrap_or(1) != 0;
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(5).clamp(1, 16) as usize;
            game::spawn_random_enemy(amount.clamp(1, 10) as i32, whole_game, bosses, only_bosses, replace_oldest, limit)
        }
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
    fn spawn_is_unavailable_once_the_session_is_safe() {
        let snapshot = crate::game::Snapshot {
            exe_match: true,
            session_known: true,
            offline: true,
            in_game: true,
            character_name: "Ash".to_string(),
        };
        assert!(gate_snapshot(&snapshot).is_ok());
        match dispatch("spawn_random_enemy", &serde_json::json!({})) {
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
