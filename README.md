# DS3 Crowd Control

Lets Twitch chat and Discord viewers mess with an offline game of Dark Souls III. On Twitch, viewers trigger events with channel point redeems, Bits or gift subs; on Discord, with a panel of buttons in a channel. The streamer can also fire any event by hand.

It has two parts: a Windows manager app where you set everything up, and `ds3mod.dll`, which the manager loads into the game to carry out the events. Events only run in an offline session with a character loaded. There is no online play and no anti-cheat bypass.

Written with AI: the code was written by Claude (Anthropic) working with the author, with some earlier work by Grok (xAI).

> **Not yet tested on a real Affiliate or Partner channel.** The Twitch part has only been tested with the [Twitch CLI](https://dev.twitch.tv/docs/cli/)'s fake events, because creating channel point redeems needs an Affiliate or Partner channel. If you use it on one, please [report any problems](https://github.com/MagnusMagnuson/ds3-crowd-control/issues) you run into.

## Setup

You need 64-bit Windows 10 or 11 and the Steam version of [Dark Souls III](https://store.steampowered.com/app/374320/DARK_SOULS_III/), updated to 1.15.2. Nothing else has to be installed: the download includes everything it needs, including .NET.

1. Download `ds3-crowd-control-v<version>.zip` from the [Releases page](https://github.com/MagnusMagnuson/ds3-crowd-control/releases) and unzip it into its own folder.
2. Start `Ds3CrowdControl.Manager.exe`. Keep `ds3mod.dll` in the same folder.
3. Launch Dark Souls III and choose to play offline.
4. Load a character, then press **Attach to Dark Souls III** on the General page.

Two warnings you may see, because the app is not signed and loads a DLL into the game:

- Windows SmartScreen may say "Windows protected your PC". Click **More info**, then **Run anyway**.
- Some antivirus programs flag tools that load a DLL into another program. If yours removes `ds3mod.dll` or blocks the manager, allow the folder.

Settings, logs, the Twitch sign-in and the Discord bot token are kept in `%AppData%\Ds3CrowdControl`, so a new version can go in a new folder.

### Building it yourself

This is only needed to change the code. You need:

- the [.NET 8 SDK](https://dotnet.microsoft.com/download/dotnet/8.0)
- [Rust](https://rustup.rs) with its default Windows (MSVC) toolchain
- the Microsoft C++ build tools, which Rust needs to link the mod: install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/) with the "Desktop development with C++" workload. The Rust installer offers to do this for you.

The first build downloads its packages, so it needs an internet connection.

- `dotnet build Ds3CrowdControl.sln` builds the manager and `ds3mod.dll` into `src/Ds3CrowdControl.Manager/bin`.
- `powershell -ExecutionPolicy Bypass -File scripts\package.ps1 -Version 0.1.0` builds the release zip in `dist`.

## Events

Each event can be fired with **Test** on the Presets page, or by Twitch and the Discord panel once it is turned on there. Most have options on the Presets page, such as how long they last.

| Event | What it does |
|---|---|
| Add or remove souls | Gives or takes souls; a negative amount takes them. With **Scale with level**, the amount grows with what your next level costs: it is exactly the amount at level 10, about 18 times it at level 50 and 74 times at level 100. |
| Give Ember | Gives you Embers, which you use to become embered. |
| Use selected item | Uses the item in your selected quick slot. Needs a controller. |
| Kill player | A real death, with the normal respawn. |
| Fake Death | Plays a real death, grey screen and YOU DIED, then stands you back up where you were, with your HP and souls. |
| Show YOU DIED | Only the banner and its sound. |
| 1 HP | Sets HP to 1. |
| 1 Hit Run | Holds HP at 1 for a while, so any hit kills. |
| Set health to 50%, Set health to random % | Sets HP to a share of the maximum. |
| Invincible | No hit damage for a while. |
| Empty Stamina | Empties stamina once. On a ladder, you fall off. |
| No Stamina | Keeps stamina empty for a while. |
| Decrease Speed, Increase Speed | Slow down or speed up the player for a while. |
| Freeze | Freezes you in place for a few seconds. The camera still moves. |
| Invert Controls | Flips the controller sticks for a while. |
| Pacifist | No attacking, blocking or using items for a while. Use selected item still works. Needs a controller. |
| Blind | Fades to black for a few seconds. |
| Oversaturate | Pushes the colours far too high for a while. |
| Random Weapon | Gives and equips a random weapon in the right hand. |
| Random Armor | Gives and equips a random piece in every armor slot. |
| Temporary Armor | A random outfit for a while, then your own armor comes back. |
| Shuffle Stats | Swaps your stats around without changing your level. By default it stays that way; set a time to get your stats back. |
| Bombardier | Takes your weapons away, dresses you in the Mirrah Chain Set and gives you endless Black Firebombs, then puts everything back. |
| Firestarter | Takes your weapons away and gives you a Pyromancy Flame, the Fire Witch Set and every pyromancy, then puts everything back. |
| Ooga Booga | Takes your weapons and armor away and gives you a fully upgraded Club with no stat requirements and War Cry's attack boost, then puts everything back. |
| Big Ooga Booga | The same with a fully upgraded Great Club. |
| Spawn Random Enemy | Spawns regular enemies near you, from this area or the whole game. |
| Spawn Random Boss | Spawns a boss near you, from this area or the whole game. |
| Spawn Anything | Spawns one random enemy or boss, or sometimes an NPC invader. |
| Spawn NPC Invader | A named NPC invader such as Havel, Hodrick or Kirk appears near you as a Dark Spirit, with its own gear. The game announces the invasion, naming the viewer who sent it if you want. |
| Spawn NPC Ally | A named NPC is summoned near you as a white phantom that fights on your side. The game announces the summon, naming the viewer who sent it if you want. |
| Mimic Chests | Three chests appear in front of you. One holds a good weapon or ring; the other two are Mimics. Open one with the interact button (A or E). Attacking a chest wakes all three and wins nothing. |

**Show developer tools** on the General page also adds testing effects to the Presets page. They never show on the Twitch or Discord page, so they cannot become Twitch rewards or Discord buttons: Toggle invincibility, Toggle invisibility (enemies neither see nor hear you), Dump phantoms, Dump player data, Watch item button, the Mimic and animation test tools, and the performance tools. **Toggle performance measuring** starts timing every frame and every part of the mod; **Performance report** then shows frame times, the mod's share of each slow frame and how busy the game's threads are, with the full report in `ds3mod.log`. Measuring is off until you switch it on.

Use selected item presses X, which is the use-item button on the game's default controls. If the controls were changed, turn on **Show developer tools**, run **Watch item button** and press the use-item button once, and the mod learns the new button.

## Manager pages

- **General:** launch and attach to the game, pause all events, set a gap between events and a limit on waiting events, restore gear snapshots, and save logs for support.
- **Twitch:** connect Twitch, set a redeem name prefix, and turn Redeem, Bits and Gift subs on or off for each event with their amounts. **Sync** switches all three together.
- **Discord:** connect your bot, choose the panel's channel, set the cooldowns, and choose which events get a button.
- **Presets:** rename events, set their options, and test them.

Changes save at once.

The log and the queue stay at the bottom of every page. The queue shows who triggered each event. Untick **Show queue** if the player does not want to see what is coming. **Clear queue** removes waiting events and refunds waiting redeems. Pausing keeps waiting events and runs them when you resume.

## Twitch

Channel points, Bits and gift subs need an Affiliate or Partner channel, signed in as the broadcaster. On the Twitch page, press **Connect Twitch** and approve the code in the browser, then turn triggers on in the table below it.

This has not been tested on a real Affiliate or Partner channel yet, only with the Twitch CLI's fake events (see below). Creating, changing and hiding redeems and refunding them are the parts most likely to need a fix, so please [report any problems](https://github.com/MagnusMagnuson/ds3-crowd-control/issues).

- **Redeem:** the manager creates its own channel point redeems on the channel, using the event name and the cost and cooldown from the Twitch page. The cooldown only limits redeems: Bits, gift subs and Test always run, since Twitch cannot refund Bits or gift subs. Twitch does not allow two redeems with the same name, so if another tool already has one called Blind or Freeze, set a **Redeem name prefix** on the Twitch page, such as `DS3: `. It goes in front of every redeem name, and changing it renames the redeems already made. Turning one off hides it on Twitch and never deletes it, so a custom icon stays. Redeems that fail are refunded, and so are redeems past the waiting limit. All redeems are hidden while the manager is closed.
- **Bits:** a cheer of exactly the set amount runs the event. Power-ups count too.
- **Gift subs:** a gift of exactly the set number of subs runs the event, at any tier.

If several events share an amount, one of them is picked at random. Twitch cannot refund Bits or gift subs, so if one of those events fails or is cleared from the queue, the viewer does not get them back.

### What connecting to Twitch does

- **Sign-in:** you sign in on Twitch's own website at twitch.tv/activate and enter the code the manager shows. The manager never sees your password. Twitch then gives it a key that allows only the permissions below.
- **Permissions:**
  - *Manage channel point redeems:* create, change and hide the event redeems, and mark redemptions as done or refunded. Twitch only lets an app edit redeems it created itself, so your own redeems are never touched.
  - *Read Bits:* see who cheered and how many Bits.
  - *Read subscriptions:* see gift subs. Twitch bundles reading the subscriber list into this permission, but the manager never asks for it.
- **What it cannot do:** read or write chat, ban anyone, change the stream title or category, or see your email, payouts or stream key.
- **Where data goes:** the manager talks only to Twitch's own servers. There is no other server in between, and nothing is sent anywhere else.
- **On your PC:** the key is stored encrypted, readable only by your Windows account, and is never put in the logs or the support zip.
- **Disconnect Twitch:** hides the redeems, cancels the key at Twitch and deletes it from your PC. If Twitch cannot be reached at that moment, the log says so, and you can remove the app yourself at twitch.tv/settings/connections.

### Testing without an Affiliate channel

The Twitch CLI can send fake events on your own PC, so everything except creating real redeems can be tested on any channel.

1. Install it with `winget install Twitch.TwitchCLI`.
2. Run `twitch event websocket start-server` and leave it open.
3. On the General page, tick **Show developer tools**. Then, on the Twitch page, tick **Use Twitch CLI test server**.
4. Turn on the triggers you want on the Twitch page, then send events from a second terminal:
   - `twitch event trigger add-redemption --transport=websocket --item-name="Blind"`
   - `twitch event trigger cheer --transport=websocket -C 100`
   - `twitch event trigger channel-gift --transport=websocket -C 5`

For a redeem, `--item-name` must match the event name.

## Discord

Viewers on a Discord server press buttons in a channel to trigger events. Pressing is free, so cooldowns are what keep it in check: each event has its own cooldown, shared by everyone, and each viewer waits a while after pressing any button. A press that fails, for example because no character is loaded, gives both back. While an event is on cooldown, its button is grey and the panel lists when it is ready again, with a countdown that Discord keeps up to date by itself.

Pressing a button also opens the viewer's own panel: a message in the same channel that only they can see. It says what happened with their last press, such as "Blind is queued" and then "Blind ran", and has all the buttons again, grey while their own cooldown or the event's runs, with countdowns. Pressing a button there changes that same message, so each viewer has one personal panel at a time, and the buttons come back by themselves when a cooldown ends. With more than 25 events it has pages. Discord only lets a bot change such a message for 15 minutes after the last press, so the bot removes a personal panel after 14 minutes without a press; pressing on the public panel opens a fresh one.

> **Not yet tested on a real server.** The Discord part has been tested against a fake Discord only. If you use it, please [report any problems](https://github.com/MagnusMagnuson/ds3-crowd-control/issues).

Each streamer uses a bot of their own, which takes a few minutes to set up once:

1. Open the [Discord Developer Portal](https://discord.com/developers/applications). The first time, Discord asks what brings you there: choose **Build a Bot**, then **Continue to Portal**. Create a new application (the button says **New Application** or **Create**) and give it a name, such as `DS3 Crowd Control`. On the General Information page that follows, nothing has to be filled in. Leave **Interactions Endpoint URL** empty: with an address there, Discord sends button presses to that address instead of to the manager.
2. On the **Bot** page, choose **Reset Token** and copy the token. It is shown only once. While there, you can turn off **Public Bot**, so only you can add it to servers.
3. On the **OAuth2** page, open the **URL Generator**, tick the scope `bot` and the permissions **View Channels** and **Send Messages**, then open the link it makes and pick your server. Adding a bot needs the Manage Server permission on that server.
4. In the manager, on the Discord page, paste the token and press **Save token**. The manager connects and stays connected whenever it runs.
5. In Discord, turn on Developer Mode (User Settings, Advanced), right-click the channel for the panel and choose **Copy Channel ID**. Paste it into **Panel channel id**.
6. Tick the events that should get a button, then press **Post or update panel**.

After that, the panel follows your changes by itself. A message holds up to 25 buttons, so with more events the panel takes two messages. When you close the manager, the buttons turn grey and the panel says it is offline; they come back when the manager starts again. If the manager does not close normally, the buttons stay, and pressing one shows Discord's "This interaction failed".

### What connecting to Discord does

- **The token** is the bot's own key, not yours. Anyone who has it can act as the bot, so do not share it. The manager stores it encrypted, readable only by your Windows account, and never puts it in the logs or the support zip. **Remove token** deletes it from your PC. To cancel it completely, reset it in the Developer Portal.
- **What the bot does:** it posts and edits its own panel messages in the channel you choose, and answers button presses. It does not read the channel's messages or anything else on the server, and it asks Discord for no special permissions.
- **Viewers:** the manager only sees who pressed a button, and shows their name on that server in the queue and, if you want, in the game's invasion and summon messages.
- **Where data goes:** the manager talks only to Discord's own servers. There is no other server in between.

## If something goes wrong

The mod saves your equipped weapons and armor every 5 minutes, taking turns between two snapshots, and skips it while a temporary swap is active. If gear ends up wrong, press **Restore** next to a snapshot on the General page. It puts that gear back on and gives back any piece that is no longer in the inventory, at the same upgrade level. A snapshot only restores onto the character it was taken from.

Press **Save logs for support** at the bottom of the General page. It saves `ds3-crowd-control-logs-<date>.zip` to the Desktop and opens the folder, ready to send. It holds the logs and your events and settings, with your Windows user name and PC name taken out, since many people use their real name for them. It never holds your Twitch sign-in. If the game crashed, tick **Include the game's crash dump** first to add the newest Dark Souls III crash dump from the last week. A crash dump is a copy of the game's memory and can contain your Windows user name and things like your Steam account name, so it is left out unless you tick it.

Temporary Armor, Bombardier and a timed Shuffle Stats survive a crash or a quit. Attach the mod again and load the same character, and your own gear or stats come back. A stat shuffle is not undone if you levelled up in the meantime, so no levels are lost.

### Reporting a problem

Open an issue on the [Issues page](https://github.com/MagnusMagnuson/ds3-crowd-control/issues) and say what you did, what you expected and what happened instead. Attach the zip from **Save logs for support**. You need a free GitHub account to open an issue.

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
