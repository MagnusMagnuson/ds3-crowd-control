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
    public bool Enabled { get; set; } = true;
    public int CooldownSeconds { get; set; }
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

    public bool IsSafe => Connected && ExeMatch && SessionKnown && Offline && InGame;
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
            Arguments = [new EffectArgumentSchema { Name = "delta", Type = "int", Default = -100, Min = -999_999_999, Max = 999_999_999 }]
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
            Arguments = [new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 5, Min = 1, Max = 60 }]
        },
        new HandlerSchema
        {
            Id = "decrease_speed",
            Name = "Decrease Speed",
            Description = "Slows the player by a percentage for the chosen time. Adds together with Increase Speed.",
            Arguments =
            [
                new EffectArgumentSchema { Name = "percent", Type = "int", Default = 30, Min = 1, Max = 90 },
                new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 30, Min = 1, Max = 300 },
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
                new EffectArgumentSchema { Name = "seconds", Type = "int", Default = 30, Min = 1, Max = 300 },
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
            Id = "spawn_npc_invader",
            Name = "Spawn NPC invader",
            Description = "Reserved. A working invader spawn needs the game's character spawn routine, which is not called in this version.",
            Arguments = [new EffectArgumentSchema { Name = "npcParamIds", Type = "string", DefaultText = "" }]
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
    public List<HandlerSchema> Handlers { get; set; } = BuiltinHandlers.All.Select(Clone).ToList();
    public List<EffectPreset> Presets { get; set; } = [];

    public static Catalog CreateDefault()
    {
        var catalog = new Catalog();
        catalog.Presets.Add(Preset("Lose 100 souls", "adjust_souls", catalog));
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
        catalog.Presets.Add(Preset("Random Weapon", "random_weapon", catalog));
        catalog.Presets.Add(Preset("Set health to 50%", "set_health_percent", catalog));
        catalog.Presets.Add(Preset("Set health to random %", "set_health_random_percent", catalog));
        catalog.Presets.Add(Preset("Spawn random NPC invader", "spawn_npc_invader", catalog));
        catalog.Presets.Add(Preset("Dump player data", "dump_player_data", catalog));
        catalog.Presets.Add(Preset("Watch item button", "watch_item_button", catalog));
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
            Enabled = true,
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
        return loaded;
    }

    public static void Save(Catalog catalog) => Save(catalog, AppPaths.CatalogFile);

    public static void Save(Catalog catalog, string path)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, JsonSerializer.Serialize(catalog, Json));
    }
}
