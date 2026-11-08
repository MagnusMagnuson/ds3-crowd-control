# DS3 Crowd Control

Local crowd-control tool for an offline Steam copy of Dark Souls III. The WPF manager configures effects and can fire them by hand. `ds3mod.dll` runs inside `DarkSoulsIII.exe` and applies them on the game thread. Twitch redeems go through the same `EffectQueue`.

Effects run only after the mod reports an offline session with a loaded character. There is no online play and no anti-cheat bypass.

## Run

1. Build `src/Ds3CrowdControl.Manager` (that also builds `ds3mod.dll` and copies it beside the app).
2. Start the manager.
3. Launch Dark Souls III and choose offline at the title prompt.
4. Load a character, then press Attach.

The default presets are Lose 100 souls, Use selected item, Spawn Random Enemy, and Dump player data.

Held souls are the 32-bit counter at player data offset `0x74` (the 1.15.2 value used by the current Grand Archives table, and the same place the public player-info layout leaves unlabeled). Dump player data writes that block under `%AppData%\Ds3CrowdControl` if the counter needs to be checked again. Soul level is at offset `0x70`, and the Scale with level option grows the amount with the cost of the next level, taking the amount as written at level 10.

## Manager pages

- **General:** attach to the game, connect Twitch, and set Pause all effects, Gap between effects and the Waiting effects limit.
- **Events:** turn Redeem, Bits and Gift subs on and off for each event, and set their amounts. Sync links the three switches. Changes save at once, and Twitch gets one update about 2 seconds after the last click.
- **Presets:** rename events, set cooldowns and effect options, and test them. Show developer tools on the General page adds Dump player data and Watch item button here, and shows Focus game for item test, which only works while developer tools are on.

Use selected item presses X on the controller, the game's default for using an item. If the controls were changed, run Watch item button once and press the use-item button so the mod learns it.

The log and queue stay at the bottom of every page. The queue shows who triggered each effect, and Show queue hides it for a player who wants surprises. The on-screen log only names an effect once it has finished. Clear queue refunds any waiting redeems, but an effect already sent to the game can't be cleared. Pause keeps waiting effects in the queue and runs them on resume.

## Testing without an Affiliate channel

The Twitch CLI can fake events on this PC, so Bits, gift subs and redeems can be tested without an Affiliate channel. Nothing reaches Twitch, and the mod only needs the game running as usual.

1. Install it with `winget install Twitch.TwitchCLI`.
2. Run `twitch event websocket start-server` and leave it open.
3. On the General page, tick Show developer tools, then Use Twitch CLI test server.
4. Turn on the triggers you want on the Events page, then fire events from a second terminal:
   - `twitch event trigger add-redemption --transport=websocket --item-name="Blind"`
   - `twitch event trigger cheer --transport=websocket -C 100`
   - `twitch event trigger channel-gift --transport=websocket -C 5`

A redeem finds its event by `--item-name`, which must match the preset name. The manager logs whether it would be completed or refunded.

## Reporting a problem

Press **Save logs for support** on the General page. It saves `ds3-crowd-control-logs-<date>.zip` to the Desktop and opens the folder. The zip holds the manager and mod logs (the last 5 MB of each), the presets and settings, any temporary swap waiting to be undone, and the newest Dark Souls III crash dump from the last 7 days if one exists and is under 400 MB. It never holds the Twitch sign-in or the session secret.

## Crashes and quitting

Temporary Armor, Bombardier and a timed Shuffle Stats keep what they must put back in `%AppData%\Ds3CrowdControl\pending-restores.json`. After a crash or a quit, attach the mod and load that character. The swap carries on if its time has not run out, and ends as soon as the character is in the world if it has. A stat shuffle is skipped if the stat total changed, so levels are never lost.

## Twitch triggers

Channel points, Bits and gift subs need an Affiliate or Partner channel, signed in as the broadcaster. Press Connect Twitch and approve the code in the browser, then use the Events page.

- **Redeem:** the manager creates its own channel point redeems, and the preset name and cooldown become the redeem title and cooldown. Turning one off hides it on Twitch and never deletes it, so a custom icon stays. Failed effects are refunded, and so are redeems past the Waiting effects limit.
- **Bits:** only the exact amount runs an event, counting cheers and Power-ups.
- **Gift subs:** only a gift of exactly that many subs runs an event, at any tier.

If events share an amount, one is picked at random. Twitch has no refund for Bits or gift subs, so if one of those effects fails or is cleared from the queue, the viewer does not get them back. A failure is logged.
