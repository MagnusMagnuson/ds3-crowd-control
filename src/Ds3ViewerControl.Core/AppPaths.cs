namespace Ds3ViewerControl.Core;

public static class AppPaths
{
    public static string Root => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "Ds3ViewerControl");

    public static string SessionFile => Path.Combine(Root, "session.json");

    public static string CatalogFile => Path.Combine(Root, "catalog.json");

    public static string TwitchTokensFile => Path.Combine(Root, "twitch-sign-in.bin");

    public static string DiscordTokenFile => Path.Combine(Root, "discord-bot.bin");

    public static string ManagerSettingsFile => Path.Combine(Root, "manager-settings.json");

    public static string GearSnapshotsFile => Path.Combine(Root, "gear-snapshots.json");

    public static void EnsureRoot() => Directory.CreateDirectory(Root);

    /// <summary>The folder's name from when the project was called DS3 Crowd Control.</summary>
    private const string OldFolderName = "Ds3CrowdControl";

    /// <summary>
    /// Moves settings, logs and sign-ins from the folder of the old name, once. Runs before
    /// anything opens a file in either folder.
    /// </summary>
    public static void MoveOldFolder()
    {
        var old = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), OldFolderName);
        try
        {
            if (Directory.Exists(old) && !Directory.Exists(Root))
            {
                Directory.Move(old, Root);
            }
        }
        catch (IOException)
        {
            // Left where it is; the manager starts with fresh settings instead of failing.
        }
        catch (UnauthorizedAccessException)
        {
        }
    }
}
