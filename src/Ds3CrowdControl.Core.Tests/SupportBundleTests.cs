using System.IO.Compression;
using System.Text;
using Ds3CrowdControl.Core;
using Xunit;

namespace Ds3CrowdControl.Core.Tests;

public sealed class SupportBundleTests : IDisposable
{
    private readonly string _root = Path.Combine(Path.GetTempPath(), "ds3-cc-tests", Guid.NewGuid().ToString("N"));
    private readonly string _app;
    private readonly string _dumps;
    private readonly string _out;
    private readonly DateTime _now = new(2026, 11, 9, 12, 0, 0);
    private static readonly Anonymizer NoChanges = new(null, null, null);

    public SupportBundleTests()
    {
        _app = Directory.CreateDirectory(Path.Combine(_root, "app")).FullName;
        _dumps = Directory.CreateDirectory(Path.Combine(_root, "dumps")).FullName;
        _out = Path.Combine(_root, "out");
    }

    public void Dispose()
    {
        try
        {
            Directory.Delete(_root, recursive: true);
        }
        catch (IOException)
        {
        }
    }

    private Dictionary<string, string> Entries(string zipPath)
    {
        using var zip = ZipFile.OpenRead(zipPath);
        return zip.Entries.ToDictionary(
            entry => entry.FullName,
            entry =>
            {
                using var reader = new StreamReader(entry.Open());
                return reader.ReadToEnd();
            });
    }

    [Fact]
    public void Logs_and_settings_go_in_but_secrets_stay_out()
    {
        File.WriteAllText(Path.Combine(_app, "manager.log"), "manager line");
        File.WriteAllText(Path.Combine(_app, "ds3mod.log"), "mod line");
        File.WriteAllText(Path.Combine(_app, "catalog.json"), "{}");
        File.WriteAllText(Path.Combine(_app, "twitch-sign-in.bin"), "secret token");
        File.WriteAllText(Path.Combine(_app, "session.json"), "secret session");

        var entries = Entries(SupportBundle.Create(_app, _out, "game: connected", _dumps, _now, NoChanges));

        Assert.Equal("manager line", entries["manager.log"]);
        Assert.Equal("mod line", entries["ds3mod.log"]);
        Assert.True(entries.ContainsKey("catalog.json"));
        Assert.DoesNotContain(entries.Keys, name => name.Contains("twitch") || name.Contains("session"));
        Assert.DoesNotContain(entries.Values, text => text.Contains("secret"));
        Assert.Contains("game: connected", entries["summary.txt"]);
    }

    [Fact]
    public void A_long_log_keeps_only_its_end()
    {
        var log = new StringBuilder();
        log.Append('a', (int)SupportBundle.LogTailBytes);
        log.Append("the newest line");
        File.WriteAllText(Path.Combine(_app, "ds3mod.log"), log.ToString());

        var entries = Entries(SupportBundle.Create(_app, _out, "", _dumps, _now, NoChanges));

        Assert.Equal(SupportBundle.LogTailBytes, entries["ds3mod.log"].Length);
        Assert.EndsWith("the newest line", entries["ds3mod.log"]);
        Assert.Contains("last 5 MB", entries["summary.txt"]);
    }

    [Fact]
    public void Only_a_recent_game_crash_dump_is_added()
    {
        var old = Path.Combine(_dumps, "DarkSoulsIII.exe.1.dmp");
        var recent = Path.Combine(_dumps, "DarkSoulsIII.exe.2.dmp");
        File.WriteAllText(old, "old");
        File.WriteAllText(recent, "recent");
        File.WriteAllText(Path.Combine(_dumps, "explorer.exe.3.dmp"), "other program");
        File.SetLastWriteTime(old, _now.AddDays(-30));
        File.SetLastWriteTime(recent, _now.AddHours(-1));

        var entries = Entries(SupportBundle.Create(_app, _out, "", _dumps, _now, NoChanges));

        Assert.Equal("recent", entries["crash/DarkSoulsIII.exe.2.dmp"]);
        Assert.Single(entries.Keys, name => name.StartsWith("crash/"));
    }

    [Fact]
    public void The_crash_dump_stays_out_unless_asked_for()
    {
        var dump = Path.Combine(_dumps, "DarkSoulsIII.exe.2.dmp");
        File.WriteAllText(dump, "recent");
        File.SetLastWriteTime(dump, _now.AddHours(-1));

        var entries = Entries(SupportBundle.Create(_app, _out, "", null, _now, NoChanges));

        Assert.DoesNotContain(entries.Keys, name => name.StartsWith("crash/"));
        Assert.Contains("game crash dump: not included", entries["summary.txt"]);
    }

    [Fact]
    public void The_windows_user_and_pc_names_are_taken_out()
    {
        var anonymizer = new Anonymizer(@"C:\Users\Someone\", "Someone", "DESKTOP-ABC");
        File.WriteAllText(Path.Combine(_app, "manager.log"), @"logs saved to C:\Users\Someone\Desktop\a.zip");
        File.WriteAllText(Path.Combine(_app, "ds3mod.log"), @"wrote c:\users\someone\AppData\x.txt on desktop-abc");
        File.WriteAllText(Path.Combine(_app, "catalog.json"), @"{""path"":""C:\\Users\\Someone""}");

        var entries = Entries(SupportBundle.Create(_app, _out, @"manager: D:\Someone\ds3\m.exe", null, _now, anonymizer));

        Assert.Equal(@"logs saved to %USERPROFILE%\Desktop\a.zip", entries["manager.log"]);
        Assert.Equal(@"wrote %USERPROFILE%\AppData\x.txt on %COMPUTERNAME%", entries["ds3mod.log"]);
        Assert.Contains(@"manager: D:\%USERNAME%\ds3\m.exe", entries["summary.txt"]);
        Assert.Equal(@"{""path"":""%USERPROFILE%""}", entries["catalog.json"]);
        Assert.DoesNotContain(entries.Values, text => text.Contains("someone", StringComparison.OrdinalIgnoreCase));
    }
}
