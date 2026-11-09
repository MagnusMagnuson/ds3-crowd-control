using Ds3CrowdControl.Core;
using Xunit;

namespace Ds3CrowdControl.Core.Tests;

public class GearSnapshotTests
{
    private static string WriteFile(string text)
    {
        var path = Path.Combine(Path.GetTempPath(), "ds3-cc-tests", Guid.NewGuid().ToString("N"), "gear-snapshots.json");
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllText(path, text);
        return path;
    }

    [Fact]
    public void Snapshots_are_listed_newest_first()
    {
        var path = WriteFile("""
            {"slots":[
              {"taken":1000,"character":"Ash","weapons":[1,2,3,4,5,6],"armor":[1,2,3,4]},
              {"taken":1300,"character":"Ash","weapons":[1,2,3,4,5,6],"armor":[1,2,3,4]}
            ],"next":0}
            """);

        var snapshots = GearSnapshotStore.Load(path);

        Assert.Equal([1, 0], snapshots.Select(snapshot => snapshot.Slot));
        Assert.True(snapshots[0].Newest);
        Assert.False(snapshots[1].Newest);
        Assert.Contains("Ash", snapshots[0].Description);
    }

    [Fact]
    public void An_empty_slot_is_left_out()
    {
        var path = WriteFile("""{"slots":[{"taken":1000,"character":"Ash","weapons":[0,0,0,0,0,0],"armor":[0,0,0,0]},null],"next":1}""");

        Assert.Single(GearSnapshotStore.Load(path));
    }

    [Fact]
    public void A_missing_or_broken_file_means_no_snapshots()
    {
        Assert.Empty(GearSnapshotStore.Load(Path.Combine(Path.GetTempPath(), Guid.NewGuid().ToString("N"))));
        Assert.Empty(GearSnapshotStore.Load(WriteFile("not json")));
    }
}
