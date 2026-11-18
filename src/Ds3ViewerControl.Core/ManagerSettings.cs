using System.Text.Json;

namespace Ds3ViewerControl.Core;

public sealed class ManagerSettings
{
    public bool FocusGameForItemTest { get; set; }
    public bool EffectsPaused { get; set; }
    public int GapSeconds { get; set; }
    public int WaitingLimit { get; set; }
    public bool ShowQueue { get; set; } = true;
    public bool ShowDevTools { get; set; }
    public bool SyncTriggers { get; set; }
    public string RewardPrefix { get; set; } = "";
    /// <summary>The channel the Discord panel is in.</summary>
    public string DiscordChannelId { get; set; } = "";
    /// <summary>How long one Discord viewer waits after pressing any button.</summary>
    public int DiscordViewerCooldownSeconds { get; set; } = 60;
    /// <summary>The panel's messages, so it is edited in place instead of posted again.</summary>
    public List<string> DiscordPanelMessageIds { get; set; } = [];
}

public static class ManagerSettingsStore
{
    private static readonly JsonSerializerOptions Json = new()
    {
        WriteIndented = true,
        PropertyNamingPolicy = JsonNamingPolicy.CamelCase
    };

    public static ManagerSettings Load() => Load(AppPaths.ManagerSettingsFile);

    public static ManagerSettings Load(string path)
    {
        if (!File.Exists(path))
        {
            return new ManagerSettings();
        }

        try
        {
            return JsonSerializer.Deserialize<ManagerSettings>(File.ReadAllText(path), Json)
                ?? new ManagerSettings();
        }
        catch (JsonException)
        {
            return new ManagerSettings();
        }
    }

    public static void Save(ManagerSettings settings) =>
        Save(settings, AppPaths.ManagerSettingsFile);

    public static void Save(ManagerSettings settings, string path)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, JsonSerializer.Serialize(settings, Json));
    }
}
