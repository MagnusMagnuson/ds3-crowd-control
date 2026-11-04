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
            Name = "Fake death",
            Description = "Plays the death animation, then stands you back up at your previous HP and souls before the bonfire reload.",
            Arguments = []
        },
        new HandlerSchema
        {
            Id = "fake_death_staged",
            Name = "Fake death with YOU DIED",
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
        catalog.Presets.Add(Preset("Fake death", "fake_death", catalog));
        catalog.Presets.Add(Preset("Fake death with YOU DIED", "fake_death_staged", catalog));
        catalog.Presets.Add(Preset("Show YOU DIED", "show_you_died", catalog));
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
            Max = argument.Max
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
        return JsonSerializer.Deserialize<Catalog>(json, Json) ?? Catalog.CreateDefault();
    }

    public static void Save(Catalog catalog) => Save(catalog, AppPaths.CatalogFile);

    public static void Save(Catalog catalog, string path)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, JsonSerializer.Serialize(catalog, Json));
    }
}
