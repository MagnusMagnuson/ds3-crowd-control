namespace Ds3CrowdControl.Core;

public static class AppPaths
{
    public static string Root => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
        "Ds3CrowdControl");

    public static string SessionFile => Path.Combine(Root, "session.json");

    public static string CatalogFile => Path.Combine(Root, "catalog.json");

    public static string TwitchTokensFile => Path.Combine(Root, "twitch-sign-in.bin");

    public static string ManagerSettingsFile => Path.Combine(Root, "manager-settings.json");

    public static void EnsureRoot() => Directory.CreateDirectory(Root);
}
