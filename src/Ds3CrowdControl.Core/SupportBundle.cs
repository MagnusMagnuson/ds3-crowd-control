using System.IO.Compression;
using System.Text;

namespace Ds3CrowdControl.Core;

/// <summary>
/// Collects the logs and settings needed to look into a problem into one zip that is easy
/// to send. The Twitch sign-in and the session secret are never included.
/// </summary>
public static class SupportBundle
{
    /// <summary>Only the end of each log is kept, so a long-running log cannot make the zip huge.</summary>
    public const long LogTailBytes = 5 * 1024 * 1024;

    /// <summary>A game crash dump larger than this is left out and only mentioned.</summary>
    public const long MaxCrashDumpBytes = 400L * 1024 * 1024;

    private static readonly TimeSpan CrashDumpAge = TimeSpan.FromDays(7);

    private static readonly string[] Logs = ["manager.log", "ds3mod.log"];

    private static readonly string[] Settings =
        ["catalog.json", "manager-settings.json", "pending-restores.json", "gear-snapshots.json"];

    public static string DefaultCrashDumpFolder => Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData),
        "CrashDumps");

    /// <summary>Writes the zip into <paramref name="destination"/> and returns its path.</summary>
    public static string Create(string appFolder, string destination, string summary, string? crashDumpFolder, DateTime now)
    {
        Directory.CreateDirectory(destination);
        var path = Path.Combine(destination, $"ds3-crowd-control-logs-{now:yyyyMMdd-HHmmss}.zip");
        var notes = new StringBuilder(summary.TrimEnd()).AppendLine().AppendLine();
        using (var zip = ZipFile.Open(path, ZipArchiveMode.Create))
        {
            foreach (var name in Logs)
            {
                var file = Path.Combine(appFolder, name);
                if (!File.Exists(file))
                {
                    notes.AppendLine($"{name}: missing");
                    continue;
                }

                var (bytes, cut) = ReadTail(file, LogTailBytes);
                AddBytes(zip, name, bytes);
                notes.AppendLine(cut ? $"{name}: last {LogTailBytes / 1024 / 1024} MB of a longer log" : $"{name}: complete");
            }

            foreach (var name in Settings)
            {
                var file = Path.Combine(appFolder, name);
                if (File.Exists(file))
                {
                    AddBytes(zip, name, ReadShared(file));
                }
            }

            AddCrashDump(zip, crashDumpFolder, now, notes);
            AddBytes(zip, "summary.txt", Encoding.UTF8.GetBytes(notes.ToString()));
        }

        return path;
    }

    private static void AddCrashDump(ZipArchive zip, string? folder, DateTime now, StringBuilder notes)
    {
        if (folder is null || !Directory.Exists(folder))
        {
            notes.AppendLine("game crash dump: none found");
            return;
        }

        var newest = new DirectoryInfo(folder)
            .GetFiles("DarkSoulsIII.exe*.dmp")
            .Where(file => now - file.LastWriteTime < CrashDumpAge)
            .OrderByDescending(file => file.LastWriteTime)
            .FirstOrDefault();
        if (newest is null)
        {
            notes.AppendLine("game crash dump: none in the last 7 days");
            return;
        }

        if (newest.Length > MaxCrashDumpBytes)
        {
            notes.AppendLine($"game crash dump: {newest.Name} from {newest.LastWriteTime:yyyy-MM-dd HH:mm} is too large to include");
            return;
        }

        zip.CreateEntryFromFile(newest.FullName, "crash/" + newest.Name, CompressionLevel.Fastest);
        notes.AppendLine($"game crash dump: {newest.Name} from {newest.LastWriteTime:yyyy-MM-dd HH:mm}");
    }

    /// <summary>The game and the manager may be writing to these while they are read.</summary>
    private static byte[] ReadShared(string file)
    {
        using var stream = new FileStream(file, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
        using var copy = new MemoryStream();
        stream.CopyTo(copy);
        return copy.ToArray();
    }

    private static (byte[] Bytes, bool Cut) ReadTail(string file, long limit)
    {
        using var stream = new FileStream(file, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
        var cut = stream.Length > limit;
        if (cut)
        {
            stream.Seek(-limit, SeekOrigin.End);
        }

        using var copy = new MemoryStream();
        stream.CopyTo(copy);
        return (copy.ToArray(), cut);
    }

    private static void AddBytes(ZipArchive zip, string name, byte[] bytes)
    {
        using var entry = zip.CreateEntry(name, CompressionLevel.Optimal).Open();
        entry.Write(bytes);
    }
}
