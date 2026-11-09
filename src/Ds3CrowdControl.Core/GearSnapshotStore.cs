using System.Text.Json;

namespace Ds3CrowdControl.Core;

/// <summary>One gear snapshot as the manager lists it. The mod takes and restores them.</summary>
public sealed record GearSnapshotInfo(int Slot, DateTime Taken, string Character, bool Newest)
{
    public string Description =>
        $"{Taken:ddd HH:mm} · {Character}{(Newest ? " · newest" : "")}";
}

public static class GearSnapshotStore
{
    /// <summary>Reads the mod's snapshot file. A missing or broken file means there are none yet.</summary>
    public static IReadOnlyList<GearSnapshotInfo> Load(string path)
    {
        try
        {
            if (!File.Exists(path))
            {
                return [];
            }

            using var stream = new FileStream(path, FileMode.Open, FileAccess.Read, FileShare.ReadWrite | FileShare.Delete);
            using var document = JsonDocument.Parse(stream);
            var slots = document.RootElement.GetProperty("slots");
            var found = new List<(int Slot, DateTime Taken, string Character)>();
            var slot = 0;
            foreach (var entry in slots.EnumerateArray())
            {
                if (entry.ValueKind == JsonValueKind.Object)
                {
                    var taken = DateTimeOffset.FromUnixTimeSeconds(entry.GetProperty("taken").GetInt64()).LocalDateTime;
                    found.Add((slot, taken, entry.GetProperty("character").GetString() ?? ""));
                }

                slot++;
            }

            var newest = found.Count == 0 ? DateTime.MinValue : found.Max(item => item.Taken);
            return found
                .OrderByDescending(item => item.Taken)
                .Select(item => new GearSnapshotInfo(item.Slot, item.Taken, item.Character, item.Taken == newest))
                .ToList();
        }
        catch (Exception exception) when (exception is IOException or JsonException or KeyNotFoundException or InvalidOperationException)
        {
            return [];
        }
    }
}
