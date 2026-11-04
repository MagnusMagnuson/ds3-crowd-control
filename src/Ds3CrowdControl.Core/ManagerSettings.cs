using System.Text.Json;

namespace Ds3CrowdControl.Core;

public sealed class ManagerSettings
{
    public bool FocusGameForItemTest { get; set; }
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
