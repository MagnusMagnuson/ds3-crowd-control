using System.Text.Json;
using System.Text.Json.Serialization;

namespace Ds3CrowdControl.Core;

public enum EffectStatus
{
    Success,
    Failed,
    Unavailable,
    Retry
}

public sealed record EffectResult(string RequestId, EffectStatus Status, string Message);

public sealed class EffectArgumentSchema
{
    public string Name { get; set; } = "";
    public string Type { get; set; } = "int";
    public int? Default { get; set; }
    public string? DefaultText { get; set; }
    public int? Min { get; set; }
    public int? Max { get; set; }
    public string? Label { get; set; }
    public string? Hint { get; set; }
    public List<string>? Options { get; set; }
}

public sealed class HandlerSchema
{
    public string Id { get; set; } = "";
    public string Name { get; set; } = "";
    public string Description { get; set; } = "";
    public List<EffectArgumentSchema> Arguments { get; set; } = [];
}

public sealed class EffectPreset
{
    public string Id { get; set; } = Guid.NewGuid().ToString("N");
    public string Name { get; set; } = "";
    public string HandlerId { get; set; } = "";
    /// <summary>The redeem's cooldown on Twitch. Bits, gift subs and Test ignore it, since Bits and gift subs cannot be refunded.</summary>
    public int CooldownSeconds { get; set; }
    public bool RedeemEnabled { get; set; }
    public int RedeemCost { get; set; } = 500;
    public string? TwitchRewardId { get; set; }
    public bool BitsEnabled { get; set; }
    public int BitsAmount { get; set; } = 100;
    public bool GiftEnabled { get; set; }
    public int GiftAmount { get; set; } = 1;
    /// <summary>Shown as a button on the Discord panel.</summary>
    public bool DiscordEnabled { get; set; }
    /// <summary>How long everyone waits after someone presses it on Discord. Separate from Twitch's, since pressing is free.</summary>
    public int DiscordCooldownSeconds { get; set; } = 120;
    public Dictionary<string, JsonElement> Arguments { get; set; } = [];
}

public sealed class GameState
{
    public bool Connected { get; init; }
    public bool ExeMatch { get; init; }
    public bool SessionKnown { get; init; }
    public bool Offline { get; init; }
    public bool InGame { get; init; }
    public string CharacterName { get; init; } = "";

    public static GameState Disconnected { get; } = new();

    /// <summary>Everything is fine except the moment, so an effect can wait.</summary>
    public bool CanWait => Connected && ExeMatch && SessionKnown && Offline;

    public bool IsSafe => CanWait && InGame;
}

public static class BuiltinHandlers
{
    public static IReadOnlyList<HandlerSchema> All { get; } =
    [
        new HandlerSchema
        {
            Id = "adjust_souls",
            Name = "Adjust souls",
            Description = "Adds a signed amount to held souls. Negative values remove souls and the result is clamped at 0.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "delta", Type = "int", Default = -100, Min = -999_999_999, Max = 999_999_999, Label = "Souls to add", Hint = "A negative number takes souls away, for example -100." },
                new EffectArgumentSchema { Name = "scaleWithLevel", Type = "bool", Default = 0, Min = 0, Max = 1, Label = "Scale with level", Hint = "Grows with the cost of your next level. The amount is what you lose at level 10, so -100 takes about 1800 souls at level 50 and 7400 at level 100." }
            ]
        },
        new HandlerSchema
        {
            Id = "use_selected_item",
            Name = "Use selected item",
            Description = "Uses whatever item is in the current quick-item slot.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "kill_player",
            Name = "Kill player",
            Description = "Sets the local player's HP to 0 so the game performs its normal death and respawn flow.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "fake_death",
            Name = "Fake Death",
            Description = "Plays the death animation, grey screen and YOU DIED banner, fades to black, then stands you back up at your previous HP and souls.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "show_you_died",
            Name = "Show YOU DIED",
            Description = "Shows the YOU DIED banner with its sound. The player stays alive and the area does not reload.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "empty_stamina",
            Name = "Empty Stamina",
            Description = "Sets current stamina to 0. Being on a ladder with no stamina makes you fall off.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "no_stamina",
            Name = "No Stamina",
            Description = "Holds stamina empty for the chosen number of seconds, blocking sprinting, rolling and attacks.",
            Arguments = [new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 10, Min = 1, Max = 60 }]
        },
        new HandlerSchema
        {
            Id = "decrease_speed",
            Name = "Decrease Speed",
            Description = "Slows the player by a percentage for the chosen time. Adds together with Increase Speed.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "percent", Type = "int", Default = 30, Min = 1, Max = 90 },
                new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 10, Min = 1, Max = 300 },
                new EffectArgumentSchema { Name = "movementOnly", Type = "bool", Default = 0, Min = 0, Max = 1, Label = "Movement only (experimental)", Hint = "Only walking, running and rolling change speed. Attacks can still start at the changed speed, because the game reports them a moment late." }
            ]
        },
        new HandlerSchema
        {
            Id = "increase_speed",
            Name = "Increase Speed",
            Description = "Speeds up the player by a percentage for the chosen time. Adds together with Decrease Speed.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "percent", Type = "int", Default = 30, Min = 1, Max = 300 },
                new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 10, Min = 1, Max = 300 },
                new EffectArgumentSchema { Name = "movementOnly", Type = "bool", Default = 0, Min = 0, Max = 1, Label = "Movement only (experimental)", Hint = "Only walking, running and rolling change speed. Attacks can still start at the changed speed, because the game reports them a moment late." }
            ]
        },
        new HandlerSchema
        {
            Id = "one_hp",
            Name = "1 HP",
            Description = "Sets current HP to 1.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "one_hit_run",
            Name = "1 Hit Run",
            Description = "Holds HP at 1 for the chosen number of seconds, so healing does nothing and any hit kills.",
            Arguments = [new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 30, Min = 1, Max = 300 }]
        },
        new HandlerSchema
        {
            Id = "invincible",
            Name = "Invincible",
            Description = "The player takes no hit damage for the chosen number of seconds. A second use keeps the longer remaining time.",
            Arguments = [new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 30, Min = 1, Max = 300 }]
        },
        new HandlerSchema
        {
            Id = "random_weapon",
            Name = "Random Weapon",
            Description = "Gives you a random weapon and equips it in right hand slot 1.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "weaponLevel", Type = "choice", Default = 0, Min = 0, Max = 2, Label = "Weapon level", Options = ["+0", "Random", "Same as right hand 1"] },
                new EffectArgumentSchema { Name = "infusion", Type = "choice", Default = 0, Min = 0, Max = 2, Label = "Infusion", Options = ["None", "Random", "Same as right hand 1"] },
                new EffectArgumentSchema { Name = "removeOld", Type = "bool", Default = 1, Min = 0, Max = 1, Label = "Remove previous weapon", Hint = "Permanently deletes the weapon that was in right hand 1, including its upgrades. Fists are never removed." },
                new EffectArgumentSchema { Name = "includeMissingDlc", Type = "bool", Default = 0, Min = 0, Max = 1, Label = "Include weapons from DLC you do not have (experimental)", Hint = "Worked in a short test without the DLC installed, but not every weapon has been tried." },
                new EffectArgumentSchema { Name = "usableOnly", Type = "bool", Default = 1, Min = 0, Max = 1, Label = "Usable two-handed only", Hint = "Skips weapons your stats cannot use, counting Strength at 1.5x for two-handing." }
            ]
        },
        new HandlerSchema
        {
            Id = "random_armor",
            Name = "Random Armor",
            Description = "Gives you a random piece for each armor slot and equips all four.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "armorStyle", Type = "choice", Default = 0, Min = 0, Max = 2, Label = "Armor style", Hint = "Matching set picks one armor set. If it has no piece for a slot, that slot stays as it is.", Options = ["Random pieces", "Matching set", "Either at random"] },
                new EffectArgumentSchema { Name = "removeOld", Type = "bool", Default = 1, Min = 0, Max = 1, Label = "Remove previous armor", Hint = "Permanently deletes the armor you were wearing. Empty slots are skipped." },
                new EffectArgumentSchema { Name = "includeMissingDlc", Type = "bool", Default = 0, Min = 0, Max = 1, Label = "Include armor from DLC you do not have (experimental)", Hint = "Weapons from a missing DLC worked in a short test, but armor has not been tried." }
            ]
        },
        new HandlerSchema
        {
            Id = "temporary_armor",
            Name = "Temporary Armor",
            Description = "Swaps all armor slots for a random outfit, then puts your own armor back when the time is up.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 120, Min = 1, Max = 600 },
                new EffectArgumentSchema { Name = "armorStyle", Type = "choice", Default = 0, Min = 0, Max = 2, Label = "Armor style", Hint = "Matching set picks one armor set. If it has no piece for a slot, that slot stays as it is.", Options = ["Random pieces", "Matching set", "Either at random"] },
                new EffectArgumentSchema { Name = "includeMissingDlc", Type = "bool", Default = 0, Min = 0, Max = 1, Label = "Include armor from DLC you do not have (experimental)", Hint = "Weapons from a missing DLC worked in a short test, but armor has not been tried." }
            ]
        },
        new HandlerSchema
        {
            Id = "bombardier",
            Name = "Bombardier",
            Description = "Takes away all your weapons, dresses you in the Mirrah Chain Set and gives you endless Black Firebombs in a free quick slot, then puts everything back when the time is up.",
            Arguments = [new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 120, Min = 5, Max = 600 }]
        },
        new HandlerSchema
        {
            Id = "firestarter",
            Name = "Firestarter",
            Description = "Takes away your weapons and gives you a Pyromancy Flame at a random level, the Fire Witch Set and every pyromancy. As many as your slots hold are attuned, without stat requirements. Everything goes back when the time is up.",
            Arguments = [new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 120, Min = 5, Max = 600 }]
        },
        new HandlerSchema
        {
            Id = "test_mimic_chests",
            Name = "Test Mimic chests",
            Description = "Developer test: spawns three Mimics in a row in front of you, and for 90 seconds logs your animation and that of everything within 6 m, map enemies included.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "spawn", Type = "bool", Default = 1, Min = 0, Max = 1, Label = "Spawn the Mimics", Hint = "Off only logs, for example next to a real Mimic while you open it." },
                new EffectArgumentSchema { Name = "npcId", Type = "int", Default = 212000, Min = 100000, Max = 999999, Label = "NPC id", Hint = "Which Mimic row to spawn. 212000 is the High Wall Mimic." },
                new EffectArgumentSchema { Name = "thinkId", Type = "int", Default = 0, Min = 0, Max = 99999999, Label = "AI row", Hint = "0 uses the NPC's own AI." }
            ]
        },
        new HandlerSchema
        {
            Id = "play_nearest_sequence",
            Name = "Play nearest animation chain",
            Description = "Developer test: the character closest to you plays the listed animations one after another.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "animations", Type = "string", DefaultText = "3000, 3004, 4100", Label = "Animation ids", Hint = "Comma-separated, played in this order." },
                new EffectArgumentSchema { Name = "gapFrames", Type = "int", Default = 45, Min = 21, Max = 600, Label = "Frames between them", Hint = "60 frames are about a second." }
            ]
        },
        new HandlerSchema
        {
            Id = "try_vanish_effect",
            Name = "Try vanish effect",
            Description = "Developer test: the closest spawned enemy gets the chosen special effect and is removed a moment later.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "effect", Type = "int", Default = 10633, Min = 0, Max = 999999999, Label = "Effect id", Hint = "10633, 10635 and 12030 are transparency effects; 101341000 is Hidden Body; 0 for none." },
                new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 2, Min = 0, Max = 30, Label = "Seconds until removed" }
            ]
        },
        new HandlerSchema
        {
            Id = "scan_nearest_animations",
            Name = "Scan nearest animations",
            Description = "Developer test: tries a list of animation ids on the character closest to you, one every few frames, and logs which ones it really plays. Takes about 30 seconds.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "force_nearest_animation",
            Name = "Force nearest animation",
            Description = "Developer test: the character closest to you, within 6 m, plays the chosen animation.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "animation", Type = "int", Default = 2003010, Min = 0, Max = 99999999, Label = "Animation id" },
                new EffectArgumentSchema { Name = "playerAnimation", Type = "int", Default = 0, Min = 0, Max = 99999999, Label = "Player animation id", Hint = "Optional. Above 0, you play this one at the same moment." }
            ]
        },
        new HandlerSchema
        {
            Id = "mimic_chests",
            Name = "Mimic Chests",
            Description = "Three chests appear in front of you. One holds a good weapon or ring, fully upgraded; the other two are Mimics. Open one with the interact button. A wrong pick bites you and the other Mimic wakes up. Attacking a chest, or taking too long, wakes all three and wins nothing.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 30, Min = 10, Max = 600, Label = "Seconds to pick" },
                new EffectArgumentSchema { Name = "prize", Type = "choice", Default = 0, Min = 0, Max = 2, Label = "Prize", Options = ["Weapon or ring", "Weapon", "Ring"] }
            ]
        },
        new HandlerSchema
        {
            Id = "ooga_booga",
            Name = "Ooga Booga",
            Description = "Takes away your weapons and armor and gives you a Club at its highest level, without stat requirements and with War Cry's attack boost, then puts everything back when the time is up.",
            Arguments = [new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 120, Min = 5, Max = 600 }]
        },
        new HandlerSchema
        {
            Id = "big_ooga_booga",
            Name = "Big Ooga Booga",
            Description = "Takes away your weapons and armor and gives you a Great Club at its highest level, without stat requirements and with War Cry's attack boost, then puts everything back when the time is up.",
            Arguments = [new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 120, Min = 5, Max = 600 }]
        },
        new HandlerSchema
        {
            Id = "shuffle_stats",
            Name = "Shuffle Stats",
            Description = "Randomly swaps your nine stats with each other, so your level stays the same.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 0, Min = 0, Max = 600, Label = "Seconds", Hint = "How long until your real stats come back. 0 keeps the shuffle, and it saves to your character." }
            ]
        },
        new HandlerSchema
        {
            Id = "oversaturate",
            Name = "Oversaturate",
            Description = "Pushes the colours far past normal for the chosen number of seconds, for a badly over-edited look.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 30, Min = 1, Max = 300 },
                new EffectArgumentSchema { Name = "percent", Type = "int", Default = 2000, Min = 100, Max = 2000 }
            ]
        },
        new HandlerSchema
        {
            Id = "blind",
            Name = "Blind",
            Description = "Fades the screen to black, holds it for the chosen number of seconds, then fades back in.",
            Arguments = [new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 5, Min = 1, Max = 60 }]
        },
        new HandlerSchema
        {
            Id = "freeze",
            Name = "Freeze",
            Description = "Freezes the player in place for the chosen number of seconds. The camera still moves.",
            Arguments = [new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 10, Min = 1, Max = 60 }]
        },
        new HandlerSchema
        {
            Id = "invert_controls",
            Name = "Invert Controls",
            Description = "Inverts the controller sticks for the chosen number of seconds. Controller only.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 15, Min = 1, Max = 300 },
                new EffectArgumentSchema { Name = "camera", Type = "bool", Default = 1, Min = 0, Max = 1, Label = "Invert camera too", Hint = "Also flips the right stick. Off means only movement is inverted." }
            ]
        },
        new HandlerSchema
        {
            Id = "pacifist",
            Name = "Pacifist",
            Description = "No attacking, blocking or using items for the chosen number of seconds. Use selected item still works. Controller only.",
            Arguments = [new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 30, Min = 1, Max = 300 }]
        },
        new HandlerSchema
        {
            Id = "set_health_percent",
            Name = "Set health percent",
            Description = "Sets current HP to a percentage of the local player's maximum HP.",
            Arguments = [new EffectArgumentSchema { Name = "percent", Type = "int", Default = 50, Min = 1, Max = 100 }]
        },
        new HandlerSchema
        {
            Id = "set_health_random_percent",
            Name = "Set health to random percent",
            Description = "Sets current HP to a random value from 1% through 100% of maximum HP.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "spawn_random_enemy",
            Name = "Spawn Random Enemy",
            Description = "Spawns random regular enemies near you, from the current map unless Whole game is on.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "amount", Type = "int", Default = 1, Min = 1, Max = 10 },
                new EffectArgumentSchema { Name = "limit", Type = "int", Default = 5, Min = 1, Max = 16, Label = "Enemy limit", Hint = "How many spawned enemies can be active at once. The game itself stops at 16." },
                new EffectArgumentSchema { Name = "wholeGame", Type = "bool", Default = 0, Min = 0, Max = 1, Label = "Whole game (experimental)", Hint = "Picks from every area and gives enemies from elsewhere a random local AI. They may act oddly." },
                new EffectArgumentSchema { Name = "replaceOldest", Type = "bool", Default = 1, Min = 0, Max = 1, Label = "Replace oldest at limit", Hint = "The game allows 16 spawned enemies. Removes the oldest so new ones appear. Off means new ones wait for a free slot." }
            ]
        },
        new HandlerSchema
        {
            Id = "spawn_random_boss",
            Name = "Spawn Random Boss",
            Description = "Spawns a random boss near you, from the current map unless Whole game is on. Many bosses rely on map events, so some may act oddly.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "amount", Type = "int", Default = 1, Min = 1, Max = 3 },
                new EffectArgumentSchema { Name = "limit", Type = "int", Default = 5, Min = 1, Max = 16, Label = "Enemy limit", Hint = "How many spawned enemies can be active at once. The game itself stops at 16." },
                new EffectArgumentSchema { Name = "wholeGame", Type = "bool", Default = 0, Min = 0, Max = 1, Label = "Whole game (experimental)", Hint = "Picks from every area and gives enemies from elsewhere a random local AI. They may act oddly." },
                new EffectArgumentSchema { Name = "replaceOldest", Type = "bool", Default = 1, Min = 0, Max = 1, Label = "Replace oldest at limit", Hint = "The game allows 16 spawned enemies. Removes the oldest so new ones appear. Off means new ones wait for a free slot." }
            ]
        },
        new HandlerSchema
        {
            Id = "spawn_anything",
            Name = "Spawn Anything",
            Description = "Spawns one random thing near you: any enemy or boss, or sometimes an NPC invader.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "invaderChance", Type = "int", Default = 20, Min = 0, Max = 100, Label = "NPC invader chance", Hint = "The chance in percent that an NPC invader comes instead of an enemy. Invaders use the Spawn NPC Invader defaults." },
                new EffectArgumentSchema { Name = "limit", Type = "int", Default = 5, Min = 1, Max = 16, Label = "Enemy limit", Hint = "How many spawned enemies can be active at once. The game itself stops at 16." },
                new EffectArgumentSchema { Name = "wholeGame", Type = "bool", Default = 0, Min = 0, Max = 1, Label = "Whole game (experimental)", Hint = "Picks from every area and gives enemies from elsewhere a random local AI. They may act oddly." },
                new EffectArgumentSchema { Name = "replaceOldest", Type = "bool", Default = 1, Min = 0, Max = 1, Label = "Replace oldest at limit", Hint = "The game allows 16 spawned enemies. Removes the oldest so new ones appear. Off means new ones wait for a free slot." }
            ]
        },
        new HandlerSchema
        {
            Id = "spawn_npc_ally",
            Name = "Spawn NPC Ally",
            Description = "Summons one of the game's own summonable NPCs near you as a white phantom that fights on your side. The game announces the summon, with the sender's name if you want.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "npcParamIds", Type = "string", DefaultText = "", Label = "NPC ids", Hint = "Optional. Comma-separated NpcParam ids of summon NPCs. Empty picks a random one." },
                new EffectArgumentSchema { Name = "useSenderName", Type = "bool", Default = 1, Min = 0, Max = 1, Label = "Show the sender's name on the phantom", Hint = "On: the summon message and the name on lock-on show whoever triggered it. Off: both show the NPC's own name." },
                new EffectArgumentSchema { Name = "arrivalDelay", Type = "int", Default = 4, Min = 0, Max = 30, Label = "Seconds between message and arrival", Hint = "The summon message shows first, and the phantom appears this many seconds later." },
                new EffectArgumentSchema { Name = "arrivalAnimation", Type = "int", Default = 63020, Min = 0, Max = 99999, Label = "Arrival animation", Hint = "The animation the phantom appears with. 63020 is the gold-sign summon pose, 63010 the hunched stand-up of an invader, 63000 a stand-up from a crouch. 0 for none." },
                new EffectArgumentSchema { Name = "thinkParamId", Type = "int", Default = 0, Min = 0, Max = 99999999, Label = "AI row", Hint = "Optional. 0 picks the NPC's own AI. Another NpcThinkParam id forces that AI." }
            ]
        },
        new HandlerSchema
        {
            Id = "spawn_npc_invader",
            Name = "Spawn NPC Invader",
            Description = "Spawns a random named NPC invader near you as a Dark Spirit, with its own gear (Havel, Heysel, Hodrick and the rest of the known set). The game announces the invasion, with the sender's name if you want.",
            Arguments =
            [
                new EffectArgumentSchema
                {
                    Name = "npcParamIds",
                    Type = "string",
                    DefaultText = "",
                    Label = "NPC ids",
                    Hint = "Optional. Comma-separated NpcParam ids. Empty picks a random known invader."
                },
                new EffectArgumentSchema { Name = "useSenderName", Type = "bool", Default = 1, Min = 0, Max = 1, Label = "Show the sender's name on the invader", Hint = "On: the invasion message and the name on lock-on show whoever triggered it. Off: both show the NPC's own name." },
                new EffectArgumentSchema { Name = "arrivalDelay", Type = "int", Default = 4, Min = 0, Max = 30, Label = "Seconds between message and arrival", Hint = "The invasion message shows first, and the invader appears this many seconds later." },
                new EffectArgumentSchema { Name = "arrivalAnimation", Type = "int", Default = 63010, Min = 0, Max = 99999, Label = "Arrival animation", Hint = "The animation the invader appears with. 63010 is the hunched stand-up of an invader, 63020 the gold-sign summon pose, 63000 a stand-up from a crouch. 0 for none." },
                new EffectArgumentSchema { Name = "thinkParamId", Type = "int", Default = 0, Min = 0, Max = 99999999, Label = "AI row", Hint = "Optional. 0 picks the invader's own AI. Another NpcThinkParam id forces that AI." }
            ]
        },
        new HandlerSchema
        {
            Id = "dump_player_data",
            Name = "Dump player data",
            Description = "Writes the local player data block to a file, including the reported soul count.",
            Arguments = [new EffectArgumentSchema { Name = "soulCount", Type = "int", Default = 0, Min = 0, Max = 999_999_999 }]
        },
        new HandlerSchema
        {
            Id = "give_ember",
            Name = "Give Ember",
            Description = "Gives you Embers. Use one to become embered, which is needed to summon phantoms.",
            Arguments = [new EffectArgumentSchema { Name = "amount", Type = "int", Default = 1, Min = 1, Max = 99 }]
        },
        new HandlerSchema
        {
            Id = "toggle_invincible",
            Name = "Toggle invincibility",
            Description = "For testing: no hit damage until you trigger it again. It stays on through deaths, rests and warps.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "toggle_invisible",
            Name = "Toggle invisibility",
            Description = "For testing: enemies neither see nor hear you until you trigger it again.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "dump_phantoms",
            Name = "Dump phantoms",
            Description = "Writes the state of every player-type character in the area to a file (you, real NPC phantoms and spawned ones), and of the enemies within 40 metres of you.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "toggle_performance",
            Name = "Toggle performance measuring",
            Description = "For testing: starts measuring how smoothly the game runs and how much time the mod takes. Trigger it again to stop; that also writes a last report.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "performance_report",
            Name = "Performance report",
            Description = "While performance measuring is on: shows how smoothly the game ran since the last report, how much time the mod used, whether the mod was busy in any slow frame, and how busy the game's threads were. The full report goes to ds3mod.log.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "watch_item_button",
            Name = "Watch item button",
            Description = "Reads the player's controller memory for about 10 seconds. Press and release the use-item button once during that time. The game is not changed.",
            Arguments = []
        }
    ];

    public static Dictionary<string, JsonElement> DefaultArguments(HandlerSchema schema)
    {
        var args = new Dictionary<string, JsonElement>();
        foreach (var argument in schema.Arguments)
        {
            args[argument.Name] = argument.Type == "string"
                ? JsonSerializer.SerializeToElement(argument.DefaultText ?? "")
                : JsonSerializer.SerializeToElement(argument.Default ?? 0);
        }
        return args;
    }
}

public sealed class Catalog
{
    public const string SoulsPresetName = "Add or remove souls";

    /// <summary>Which changes of default values this catalog has been through.</summary>
    public const int CurrentDefaultsVersion = 2;

    public List<HandlerSchema> Handlers { get; set; } = BuiltinHandlers.All.Select(Clone).ToList();
    public List<EffectPreset> Presets { get; set; } = [];
    /// <summary>Saved catalogs from before default values changed have 0 here.</summary>
    public int DefaultsVersion { get; set; }

    /// <summary>Default values that changed in version 1: handler, argument, old value, new value.</summary>
    private static readonly (string Handler, string Argument, int Old, int New)[] DefaultChangesV1 =
    [
        ("no_stamina", "seconds", 5, 10),
        ("decrease_speed", "seconds", 30, 10),
        ("increase_speed", "seconds", 30, 10),
        ("temporary_armor", "seconds", 60, 120),
        ("oversaturate", "seconds", 15, 30),
        ("freeze", "seconds", 5, 10),
        ("bombardier", "seconds", 60, 120),
        ("firestarter", "seconds", 300, 120),
        ("ooga_booga", "seconds", 60, 120),
        ("big_ooga_booga", "seconds", 60, 120),
        ("mimic_chests", "seconds", 60, 30),
    ];

    /// <summary>
    /// Brings presets saved with old default values up to the new ones, once. A value the
    /// user changed is left alone, and so are presets they renamed.
    /// </summary>
    public bool UpgradeDefaults()
    {
        if (DefaultsVersion >= CurrentDefaultsVersion)
        {
            return false;
        }

        foreach (var preset in Presets)
        {
            var changes = DefaultsVersion < 1 ? DefaultChangesV1 : [];
            foreach (var change in changes.Where(change => change.Handler == preset.HandlerId))
            {
                if (preset.Arguments.TryGetValue(change.Argument, out var value)
                    && value.ValueKind == JsonValueKind.Number
                    && value.TryGetInt32(out var number)
                    && number == change.Old)
                {
                    preset.Arguments[change.Argument] = JsonSerializer.SerializeToElement(change.New);
                }
            }

            // The souls preset can give souls as well as take them, so its name says both.
            if (preset.HandlerId == "adjust_souls" && preset.Name is "Lose 100 souls" or "Lose souls")
            {
                preset.Name = SoulsPresetName;
            }
        }

        DefaultsVersion = CurrentDefaultsVersion;
        return true;
    }

    /// <summary>
    /// The order presets are listed in everywhere: the manager's pages, the Discord panel and
    /// the README. Related events sit together. Anything not listed, such as the developer
    /// tools, comes after, in the order it was added.
    /// </summary>
    public static readonly IReadOnlyList<string> PresetOrder =
    [
        // Souls and items
        "adjust_souls", "give_ember", "use_selected_item",
        // Death
        "kill_player", "fake_death", "show_you_died",
        // Health
        "one_hp", "one_hit_run", "set_health_percent", "set_health_random_percent", "invincible",
        // Stamina and movement
        "empty_stamina", "no_stamina", "decrease_speed", "increase_speed", "freeze",
        // Controls
        "invert_controls", "pacifist",
        // Screen
        "blind", "oversaturate",
        // Gear and stats
        "random_weapon", "random_armor", "temporary_armor", "shuffle_stats", "bombardier", "firestarter", "ooga_booga", "big_ooga_booga",
        // Spawns
        "spawn_random_enemy", "spawn_random_boss", "spawn_anything", "spawn_npc_invader", "spawn_npc_ally", "mimic_chests",
    ];

    /// <summary>Puts the presets in <see cref="PresetOrder"/>. Returns whether anything moved.</summary>
    public bool SortPresets()
    {
        int Rank(EffectPreset preset)
        {
            var index = PresetOrder.ToList().IndexOf(preset.HandlerId);
            return index < 0 ? PresetOrder.Count : index;
        }

        var sorted = Presets.OrderBy(Rank).ToList();
        if (sorted.SequenceEqual(Presets))
        {
            return false;
        }

        Presets = sorted;
        return true;
    }

    public static Catalog CreateDefault()
    {
        var catalog = new Catalog { DefaultsVersion = CurrentDefaultsVersion };
        catalog.Presets.Add(Preset(SoulsPresetName, "adjust_souls", catalog));
        catalog.Presets.Add(Preset("Use selected item", "use_selected_item", catalog));
        catalog.Presets.Add(Preset("Kill player", "kill_player", catalog));
        catalog.Presets.Add(Preset("Fake Death", "fake_death", catalog));
        catalog.Presets.Add(Preset("Show YOU DIED", "show_you_died", catalog));
        catalog.Presets.Add(Preset("Empty Stamina", "empty_stamina", catalog));
        catalog.Presets.Add(Preset("No Stamina", "no_stamina", catalog));
        catalog.Presets.Add(Preset("Decrease Speed", "decrease_speed", catalog));
        catalog.Presets.Add(Preset("Increase Speed", "increase_speed", catalog));
        catalog.Presets.Add(Preset("1 HP", "one_hp", catalog));
        catalog.Presets.Add(Preset("1 Hit Run", "one_hit_run", catalog));
        catalog.Presets.Add(Preset("Invincible", "invincible", catalog));
        catalog.Presets.Add(Preset("Random Weapon", "random_weapon", catalog));
        catalog.Presets.Add(Preset("Random Armor", "random_armor", catalog));
        catalog.Presets.Add(Preset("Temporary Armor", "temporary_armor", catalog));
        catalog.Presets.Add(Preset("Bombardier", "bombardier", catalog));
        catalog.Presets.Add(Preset("Firestarter", "firestarter", catalog));
        catalog.Presets.Add(Preset("Ooga Booga", "ooga_booga", catalog));
        catalog.Presets.Add(Preset("Mimic Chests", "mimic_chests", catalog));
        catalog.Presets.Add(Preset("Big Ooga Booga", "big_ooga_booga", catalog));
        catalog.Presets.Add(Preset("Shuffle Stats", "shuffle_stats", catalog));
        catalog.Presets.Add(Preset("Oversaturate", "oversaturate", catalog));
        catalog.Presets.Add(Preset("Blind", "blind", catalog));
        catalog.Presets.Add(Preset("Freeze", "freeze", catalog));
        catalog.Presets.Add(Preset("Invert Controls", "invert_controls", catalog));
        catalog.Presets.Add(Preset("Pacifist", "pacifist", catalog));
        catalog.Presets.Add(Preset("Set health to 50%", "set_health_percent", catalog));
        catalog.Presets.Add(Preset("Set health to random %", "set_health_random_percent", catalog));
        catalog.Presets.Add(Preset("Spawn Random Enemy", "spawn_random_enemy", catalog));
        catalog.Presets.Add(Preset("Spawn Random Boss", "spawn_random_boss", catalog));
        catalog.Presets.Add(Preset("Spawn Anything", "spawn_anything", catalog));
        catalog.Presets.Add(Preset("Spawn NPC Invader", "spawn_npc_invader", catalog));
        catalog.Presets.Add(Preset("Spawn NPC Ally", "spawn_npc_ally", catalog));
        catalog.Presets.Add(Preset("Dump player data", "dump_player_data", catalog));
        catalog.Presets.Add(Preset("Give Ember", "give_ember", catalog));
        catalog.Presets.Add(Preset("Toggle invincibility", "toggle_invincible", catalog));
        catalog.Presets.Add(Preset("Toggle invisibility", "toggle_invisible", catalog));
        catalog.Presets.Add(Preset("Dump phantoms", "dump_phantoms", catalog));
        catalog.Presets.Add(Preset("Toggle performance measuring", "toggle_performance", catalog));
        catalog.Presets.Add(Preset("Performance report", "performance_report", catalog));
        catalog.Presets.Add(Preset("Watch item button", "watch_item_button", catalog));
        catalog.SortPresets();
        return catalog;
    }

    public HandlerSchema? FindHandler(string handlerId) =>
        Handlers.FirstOrDefault(handler => handler.Id == handlerId);

    public EffectPreset? FindPreset(string presetId) =>
        Presets.FirstOrDefault(preset => preset.Id == presetId);

    public EffectPreset? EnsurePreset(string name, string handlerId)
    {
        if (Presets.Any(preset => preset.HandlerId == handlerId) || FindHandler(handlerId) is null)
        {
            return null;
        }

        var preset = Preset(name, handlerId, this);
        Presets.Add(preset);
        return preset;
    }

    public List<EffectPreset> RemoveOrphanPresets()
    {
        var orphans = Presets.Where(preset => FindHandler(preset.HandlerId) is null).ToList();
        Presets.RemoveAll(orphans.Contains);
        return orphans;
    }

    public void ReplaceHandlers(IReadOnlyList<HandlerSchema> handlers)
    {
        if (handlers.Count == 0)
        {
            return;
        }
        Handlers = handlers.Select(Clone).ToList();
    }

    private static EffectPreset Preset(string name, string handlerId, Catalog catalog)
    {
        var handler = catalog.FindHandler(handlerId) ?? throw new InvalidOperationException(handlerId);
        return new EffectPreset
        {
            Name = name,
            HandlerId = handlerId,
            CooldownSeconds = 0,
            Arguments = BuiltinHandlers.DefaultArguments(handler)
        };
    }

    private static HandlerSchema Clone(HandlerSchema schema) => new()
    {
        Id = schema.Id,
        Name = schema.Name,
        Description = schema.Description,
        Arguments = schema.Arguments.Select(argument => new EffectArgumentSchema
        {
            Name = argument.Name,
            Type = argument.Type,
            Default = argument.Default,
            DefaultText = argument.DefaultText,
            Min = argument.Min,
            Max = argument.Max,
            Label = argument.Label,
            Hint = argument.Hint,
            Options = argument.Options?.ToList()
        }).ToList()
    };
}

public static class CatalogStore
{
    private static readonly JsonSerializerOptions Json = new()
    {
        WriteIndented = true,
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase,
        DefaultIgnoreCondition = JsonIgnoreCondition.WhenWritingNull
    };

    public static Catalog LoadOrCreate() => LoadOrCreate(AppPaths.CatalogFile);

    public static Catalog LoadOrCreate(string path)
    {
        if (!File.Exists(path))
        {
            var created = Catalog.CreateDefault();
            Save(created, path);
            return created;
        }

        var json = File.ReadAllText(path);
        var loaded = JsonSerializer.Deserialize<Catalog>(json, Json) ?? Catalog.CreateDefault();
        // A saved schema can be older than this build, and the game may not be running yet.
        loaded.ReplaceHandlers(BuiltinHandlers.All);
        var upgraded = loaded.UpgradeDefaults();
        if (loaded.SortPresets() | upgraded)
        {
            Save(loaded, path);
        }

        return loaded;
    }

    public static void Save(Catalog catalog) => Save(catalog, AppPaths.CatalogFile);

    public static void Save(Catalog catalog, string path)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, JsonSerializer.Serialize(catalog, Json));
    }
}
