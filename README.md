# DS3 Crowd Control

Local crowd-control tool for an offline Steam copy of Dark Souls III. The WPF manager configures effects and can fire them by hand. `ds3mod.dll` runs inside `DarkSoulsIII.exe` and applies them on the game thread. A later Twitch source can call the same `EffectQueue` through `ITriggerSource`.

Effects run only after the mod reports an offline session with a loaded character. There is no online play and no anti-cheat bypass.

## Run

1. Build `src/Ds3CrowdControl.Manager` (that also builds `ds3mod.dll` and copies it beside the app).
2. Start the manager.
3. Launch Dark Souls III and choose offline at the title prompt.
4. Load a character, then press Attach.

The default presets are Lose 100 souls, Use selected item, Spawn random NPC invader, and Dump player data. Spawn is registered and returns unavailable until a later version calls the game's character spawn routine.

Held souls are the 32-bit counter at player data offset `0x74` (the 1.15.2 value used by the current Grand Archives table, and the same place the public player-info layout leaves unlabeled). Dump player data writes that block under `%AppData%\Ds3CrowdControl` if the counter needs to be checked again.
