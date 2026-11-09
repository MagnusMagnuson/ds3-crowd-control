# DS3 Crowd Control

Lets Twitch chat mess with an offline game of Dark Souls III. Viewers trigger events with channel point redeems, Bits or gift subs, and the streamer can also fire any event by hand.

It has two parts: a Windows manager app where you set everything up, and `ds3mod.dll`, which the manager loads into the game to carry out the events. Events only run in an offline session with a character loaded. There is no online play and no anti-cheat bypass.

Written with AI: the code was written by Claude (Anthropic) working with the author, with some earlier work by Grok (xAI).

## Setup

You need 64-bit Windows 10 or 11 and the Steam version of [Dark Souls III](https://store.steampowered.com/app/374320/DARK_SOULS_III/), updated to 1.15.2. Nothing else has to be installed: the download includes everything it needs, including .NET.

1. Download `ds3-crowd-control-v<version>.zip` from the [Releases page](https://github.com/MagnusMagnuson/ds3-crowd-control/releases) and unzip it into its own folder.
2. Start `Ds3CrowdControl.Manager.exe`. Keep `ds3mod.dll` in the same folder.
3. Launch Dark Souls III and choose to play offline.
4. Load a character, then press **Attach to Dark Souls III** on the General page.

Two warnings you may see, because the app is not signed and loads a DLL into the game:

- Windows SmartScreen may say "Windows protected your PC". Click **More info**, then **Run anyway**.
- Some antivirus programs flag tools that load a DLL into another program. If yours removes `ds3mod.dll` or blocks the manager, allow the folder.

Settings, logs and the Twitch sign-in are kept in `%AppData%\Ds3CrowdControl`, so a new version can go in a new folder.

### Building it yourself

This is only needed to change the code. You need:

- the [.NET 8 SDK](https://dotnet.microsoft.com/download/dotnet/8.0)
- [Rust](https://rustup.rs) with its default Windows (MSVC) toolchain
- the Microsoft C++ build tools, which Rust needs to link the mod: install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload. The Rust installer offers to do this for you.

The first build downloads its packages, so it needs an internet connection.

- `dotnet build Ds3CrowdControl.sln` builds the manager and `ds3mod.dll` into `src/Ds3CrowdControl.Manager/bin`.
- `powershell -ExecutionPolicy Bypass -File scripts\package.ps1 -Version 0.1.0` builds the release zip in `dist`.

## Events

Each event can be fired with **Test** on the Presets page, or by Twitch once its triggers are turned on. Most have options on the Presets page, such as how long they last.

| Event | What it does |
|---|---|
| Lose 100 souls | Takes souls away. With **Scale with level**, the amount grows with what your next level costs: it is exactly the amount at level 10, about 18 times it at level 50 and 74 times at level 100. |
| Use selected item | Uses the item in your selected quick slot. Needs a controller. |
| Kill player | A real death, with the normal respawn. |
| Fake Death | Plays a real death, grey screen and YOU DIED, then stands you back up where you were, with your HP and souls. |
| Show YOU DIED | Only the banner and its sound. |
| Empty Stamina | Empties stamina once. On a ladder, you fall off. |
| No Stamina | Keeps stamina empty for a while. |
| Decrease Speed, Increase Speed | Slow down or speed up the player for a while. |
| 1 HP | Sets HP to 1. |
| 1 Hit Run | Holds HP at 1 for a while, so any hit kills. |
| Invincible | No hit damage for a while. |
| Set health to 50%, Set health to random % | Sets HP to a share of the maximum. |
| Random Weapon | Gives and equips a random weapon in the right hand. |
| Random Armor | Gives and equips a random piece in every armor slot. |
| Temporary Armor | A random outfit for a while, then your own armor comes back. |
| Bombardier | Takes your weapons away, dresses you in the Mirrah Chain Set and gives you endless Black Firebombs, then puts everything back. |
| Shuffle Stats | Swaps your stats around without changing your level. By default it stays that way; set a time to get your stats back. |
| Spawn Random Enemy | Spawns enemies near you, from this area or the whole game, with an option for bosses. |
| Blind | Fades to black for a few seconds. |
| Oversaturate | Pushes the colours far too high for a while. |
| Freeze | Freezes you in place for a few seconds. The camera still moves. |
| Invert Controls | Flips the controller sticks for a while. |

Use selected item presses X, which is the use-item button on the game's default controls. If the controls were changed, turn on **Show developer tools**, run **Watch item button** and press the use-item button once, and the mod learns the new button.

## Manager pages

- **General:** attach to the game, connect Twitch, pause all events, set a gap between events and a limit on waiting redeems, and save logs for support.
- **Events:** turn Redeem, Bits and Gift subs on or off for each event and set their amounts. **Sync** switches all three together. Changes save at once.
- **Presets:** rename events, set cooldowns and options, and test them.

The log and the queue stay at the bottom of every page. The queue shows who triggered each event. Untick **Show queue** if the player does not want to see what is coming. **Clear queue** removes waiting events and refunds waiting redeems. Pausing keeps waiting events and runs them when you resume.

## Twitch

Channel points, Bits and gift subs need an Affiliate or Partner channel, signed in as the broadcaster. Press **Connect Twitch** and approve the code in the browser, then turn triggers on in the Events page.

- **Redeem:** the manager creates its own channel point redeems on the channel, using the event name, the cost from the Events page and the cooldown from the Presets page. Turning one off hides it on Twitch and never deletes it, so a custom icon stays. Redeems that fail are refunded, and so are redeems past the waiting limit. All redeems are hidden while the manager is closed.
- **Bits:** a cheer of exactly the set amount runs the event. Power-ups count too.
- **Gift subs:** a gift of exactly the set number of subs runs the event, at any tier.

If several events share an amount, one of them is picked at random. Twitch cannot refund Bits or gift subs, so if one of those events fails or is cleared from the queue, the viewer does not get them back.

### Testing without an Affiliate channel

The Twitch CLI can send fake events on your own PC, so everything except creating real redeems can be tested on any channel.

1. Install it with `winget install Twitch.TwitchCLI`.
2. Run `twitch event websocket start-server` and leave it open.
3. On the General page, tick **Show developer tools**, then **Use Twitch CLI test server**.
4. Turn on the triggers you want in the Events page, then send events from a second terminal:
   - `twitch event trigger add-redemption --transport=websocket --item-name="Blind"`
   - `twitch event trigger cheer --transport=websocket -C 100`
   - `twitch event trigger channel-gift --transport=websocket -C 5`

For a redeem, `--item-name` must match the event name.

## If something goes wrong

Press **Save logs for support** at the bottom of the General page. It saves `ds3-crowd-control-logs-<date>.zip` to the Desktop and opens the folder, ready to send. It holds the logs, your events and settings, and the newest Dark Souls III crash dump from the last week if there is one. It never holds your Twitch sign-in.

Temporary Armor, Bombardier and a timed Shuffle Stats survive a crash or a quit. Attach the mod again and load the same character, and your own gear or stats come back. A stat shuffle is not undone if you levelled up in the meantime, so no levels are lost.

## Credits

- **Code:** written by Claude (Anthropic) with the author, with some earlier work by Grok (xAI).
- **[fromsoftware-rs](https://github.com/vswarte/eldenring-rs)** (the `darksouls3` crate) by vswarte and contributors: game structures and helpers the mod is built on.
- **[The Grand Archives Dark Souls III table](https://github.com/The-Grand-Archives/Dark-Souls-III-CT-TGA):** reference for player data offsets, such as held souls.
- **[DS3 Item Randomiser and Auto-Equip](https://www.nexusmods.com/darksouls3/mods/2004)** by LukeYui ([source](https://github.com/LukeYui/DS3-Item-Randomiser-OS)): reference for how other mods equip items. No code was copied.
- **[commands.gg](https://commands.gg/ds3/items):** item IDs, such as Black Firebomb and the Mirrah Chain Set.
- **[Dark Souls III Fextralife wiki](https://darksouls3.wiki.fextralife.com/):** level-up costs and armor set details.
- **[West Games Dark Souls leveling calculator](https://west-games.com/dark-souls-leveling-calculator/):** the level-up cost formula.
- **[DSAnimStudio](https://github.com/Meowmaritus/DSAnimStudio)** by Meowmaritus: animation event names that helped track down the Fake Death hit bug.
- **[Twitch developer documentation](https://dev.twitch.tv/docs/)** and the **[Twitch CLI](https://github.com/twitchdev/twitch-cli)**.

Libraries and their licenses are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).

Dark Souls is a trademark of Bandai Namco Entertainment and FromSoftware. This is an unofficial fan project and is not affiliated with or endorsed by them.
