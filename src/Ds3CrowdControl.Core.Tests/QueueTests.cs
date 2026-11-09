using System.Net.Sockets;
using System.Text;
using System.Text.Json;
using Ds3CrowdControl.Core;
using Xunit;

namespace Ds3CrowdControl.Core.Tests;

public class CatalogTests
{
    [Fact]
    public void Loading_replaces_a_saved_schema_from_an_older_build()
    {
        var path = Path.Combine(Path.GetTempPath(), "ds3-cc-tests", Guid.NewGuid().ToString("N"), "catalog.json");
        var saved = Catalog.CreateDefault();
        saved.FindHandler("decrease_speed")!.Arguments.RemoveAll(argument => argument.Name == "movementOnly");
        CatalogStore.Save(saved, path);

        var loaded = CatalogStore.LoadOrCreate(path);

        Assert.Contains(loaded.FindHandler("decrease_speed")!.Arguments, argument => argument.Name == "movementOnly");
    }

    [Fact]
    public void Presets_without_a_handler_are_removed()
    {
        var catalog = Catalog.CreateDefault();
        catalog.Handlers.RemoveAll(handler => handler.Id == "kill_player");

        var removed = catalog.RemoveOrphanPresets();

        Assert.Equal("kill_player", Assert.Single(removed).HandlerId);
        Assert.DoesNotContain(catalog.Presets, preset => preset.HandlerId == "kill_player");
    }

    [Fact]
    public void DefaultCatalogRoundTrips()
    {
        var path = Path.Combine(Path.GetTempPath(), "ds3-cc-tests", Guid.NewGuid().ToString("N"), "catalog.json");
        var catalog = Catalog.CreateDefault();
        CatalogStore.Save(catalog, path);
        var loaded = CatalogStore.LoadOrCreate(path);

        Assert.Contains(loaded.Presets, preset => preset.Name == "Lose 100 souls" && preset.HandlerId == "adjust_souls");
        Assert.Contains(loaded.Presets, preset => preset.Name == "Kill player" && preset.HandlerId == "kill_player");
        Assert.Contains(loaded.Presets, preset => preset.Name == "Fake Death" && preset.HandlerId == "fake_death");
        Assert.DoesNotContain(loaded.Presets, preset => preset.HandlerId == "fake_death_staged");
        Assert.Contains(loaded.Presets, preset => preset.Name == "Show YOU DIED" && preset.HandlerId == "show_you_died");
        Assert.Contains(loaded.Presets, preset => preset.Name == "Empty Stamina" && preset.HandlerId == "empty_stamina");
        var noStamina = loaded.Presets.Single(preset => preset.HandlerId == "no_stamina");
        Assert.Equal(5, noStamina.Arguments["seconds"].GetInt32());
        foreach (var handlerId in new[] { "decrease_speed", "increase_speed" })
        {
            var speed = loaded.Presets.Single(preset => preset.HandlerId == handlerId);
            Assert.Equal(30, speed.Arguments["percent"].GetInt32());
            Assert.Equal(30, speed.Arguments["seconds"].GetInt32());
            Assert.Equal(0, speed.Arguments["movementOnly"].GetInt32());
        }

        Assert.Contains(loaded.Presets, preset => preset.Name == "1 HP" && preset.HandlerId == "one_hp");
        var oneHitRun = loaded.Presets.Single(preset => preset.HandlerId == "one_hit_run");
        Assert.Equal(30, oneHitRun.Arguments["seconds"].GetInt32());
        var invincible = loaded.Presets.Single(preset => preset.HandlerId == "invincible");
        Assert.Equal(30, invincible.Arguments["seconds"].GetInt32());
        var randomWeapon = loaded.Presets.Single(preset => preset.Name == "Random Weapon" && preset.HandlerId == "random_weapon");
        Assert.Equal(0, randomWeapon.Arguments["weaponLevel"].GetInt32());
        Assert.Equal(0, randomWeapon.Arguments["infusion"].GetInt32());
        Assert.Equal(1, randomWeapon.Arguments["usableOnly"].GetInt32());
        Assert.Equal(1, randomWeapon.Arguments["removeOld"].GetInt32());
        Assert.Equal(0, randomWeapon.Arguments["includeMissingDlc"].GetInt32());
        var randomArmor = loaded.Presets.Single(preset => preset.Name == "Random Armor" && preset.HandlerId == "random_armor");
        Assert.Equal(1, randomArmor.Arguments["removeOld"].GetInt32());
        Assert.Equal(0, randomArmor.Arguments["armorStyle"].GetInt32());
        Assert.Equal(0, randomArmor.Arguments["includeMissingDlc"].GetInt32());
        var temporaryArmor = loaded.Presets.Single(preset => preset.Name == "Temporary Armor" && preset.HandlerId == "temporary_armor");
        Assert.Equal(60, temporaryArmor.Arguments["seconds"].GetInt32());
        var oversaturate = loaded.Presets.Single(preset => preset.Name == "Oversaturate" && preset.HandlerId == "oversaturate");
        Assert.Equal(15, oversaturate.Arguments["seconds"].GetInt32());
        Assert.Equal(2000, oversaturate.Arguments["percent"].GetInt32());
        var blind = loaded.Presets.Single(preset => preset.Name == "Blind" && preset.HandlerId == "blind");
        Assert.Equal(5, blind.Arguments["seconds"].GetInt32());
        var invert = loaded.Presets.Single(preset => preset.Name == "Invert Controls" && preset.HandlerId == "invert_controls");
        Assert.Equal(15, invert.Arguments["seconds"].GetInt32());
        Assert.Equal(1, invert.Arguments["camera"].GetInt32());
        var halfHealth = loaded.Presets.Single(preset => preset.HandlerId == "set_health_percent");
        Assert.Equal(50, halfHealth.Arguments["percent"].GetInt32());
        Assert.Contains(loaded.Presets, preset => preset.HandlerId == "set_health_random_percent");
        Assert.Contains(loaded.Presets, preset => preset.HandlerId == "spawn_random_enemy");
        var shuffle = loaded.Presets.Single(preset => preset.Name == "Shuffle Stats" && preset.HandlerId == "shuffle_stats");
        Assert.Equal(0, shuffle.Arguments["seconds"].GetInt32());
        var freeze = loaded.Presets.Single(preset => preset.Name == "Freeze" && preset.HandlerId == "freeze");
        Assert.Equal(5, freeze.Arguments["seconds"].GetInt32());
        var pacifist = loaded.Presets.Single(preset => preset.Name == "Pacifist" && preset.HandlerId == "pacifist");
        Assert.Equal(30, pacifist.Arguments["seconds"].GetInt32());
        var bombardier = loaded.Presets.Single(preset => preset.Name == "Bombardier" && preset.HandlerId == "bombardier");
        Assert.Equal(60, bombardier.Arguments["seconds"].GetInt32());
        var souls = loaded.Presets.Single(preset => preset.HandlerId == "adjust_souls");
        Assert.Equal(-100, souls.Arguments["delta"].GetInt32());
        Assert.Equal(0, souls.Arguments["scaleWithLevel"].GetInt32());
    }

    [Fact]
    public void ManagerSettingsRoundTrip()
    {
        var path = Path.Combine(
            Path.GetTempPath(),
            "ds3-cc-tests",
            Guid.NewGuid().ToString("N"),
            "manager-settings.json");
        ManagerSettingsStore.Save(
            new ManagerSettings { FocusGameForItemTest = true, EffectsPaused = true, GapSeconds = 7, WaitingLimit = 3, ShowQueue = false, ShowDevTools = true, SyncTriggers = true },
            path);

        var loaded = ManagerSettingsStore.Load(path);
        Assert.True(loaded.FocusGameForItemTest);
        Assert.True(loaded.EffectsPaused);
        Assert.Equal(7, loaded.GapSeconds);
        Assert.Equal(3, loaded.WaitingLimit);
        Assert.False(loaded.ShowQueue);
        Assert.True(loaded.ShowDevTools);
        Assert.True(loaded.SyncTriggers);
    }
}

public class QueueTests : IAsyncLifetime
{
    private readonly string _catalogPath = Path.Combine(Path.GetTempPath(), "ds3-cc-tests", Guid.NewGuid().ToString("N"), "catalog.json");
    private Catalog _catalog = null!;
    private SessionSecret _secret = null!;
    private GameBridgeServer _bridge = null!;
    private EffectQueue _queue = null!;

    public async Task InitializeAsync()
    {
        _catalog = Catalog.CreateDefault();
        CatalogStore.Save(_catalog, _catalogPath);
        _secret = SessionSecret.Create();
        _bridge = new GameBridgeServer(_secret);
        _queue = new EffectQueue(_catalog, _bridge);
        await _bridge.StartAsync(0, publishSessionFile: false);
    }

    public async Task DisposeAsync() => await _bridge.DisposeAsync();

    [Fact]
    public async Task OnlineSessionRejectsBeforeTheGameIsAsked()
    {
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: false, inGame: true));
        await WaitUntilAsync(() => _bridge.State is { Connected: true, Offline: false, SessionKnown: true });

        var preset = _catalog.Presets.Single(item => item.HandlerId == "adjust_souls");
        var result = await _queue.EnqueueAsync(preset.Id, "manual");
        Assert.Equal(EffectStatus.Unavailable, result.Status);
        Assert.Contains("offline", result.Message);

        using var timeout = new CancellationTokenSource(TimeSpan.FromMilliseconds(200));
        await Assert.ThrowsAnyAsync<OperationCanceledException>(() => ReadLineAsync(client, timeout.Token));
    }

    [Fact]
    public async Task UnknownSessionRejects()
    {
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: false, inGame: true, sessionKnown: false));
        await WaitUntilAsync(() => _bridge.State.Connected && !_bridge.State.SessionKnown);
        var preset = _catalog.Presets.Single(item => item.HandlerId == "use_selected_item");
        var result = await _queue.EnqueueAsync(preset.Id, "manual");
        Assert.Equal(EffectStatus.Unavailable, result.Status);
        Assert.Contains("unknown", result.Message);
    }

    [Fact]
    public async Task SafeSessionDeliversTriggerAndRecordsSuccess()
    {
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: true));
        await WaitUntilAsync(() => _bridge.State.IsSafe);

        var preset = _catalog.Presets.Single(item => item.HandlerId == "adjust_souls");
        var pending = _queue.EnqueueAsync(preset.Id, "manual");
        var line = await ReadLineAsync(client, new CancellationTokenSource(TimeSpan.FromSeconds(2)).Token);
        using var document = JsonDocument.Parse(line);
        Assert.Equal("trigger", document.RootElement.GetProperty("type").GetString());
        Assert.Equal("adjust_souls", document.RootElement.GetProperty("handler").GetString());
        Assert.Equal(-100, document.RootElement.GetProperty("args").GetProperty("delta").GetInt32());
        var requestId = document.RootElement.GetProperty("requestId").GetString();
        await SendAsync(client, Result(requestId!, "success", "souls 140 -> 40"));

        var result = await pending;
        Assert.Equal(EffectStatus.Success, result.Status);
        Assert.Contains("140", result.Message);
    }

    [Fact]
    public async Task SpawnPresetReturnsUnavailableFromTheMod()
    {
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: true));
        await WaitUntilAsync(() => _bridge.State.IsSafe);
        var preset = _catalog.Presets.Single(item => item.HandlerId == "spawn_random_enemy");
        var pending = _queue.EnqueueAsync(preset.Id, "manual");
        var line = await ReadLineAsync(client, new CancellationTokenSource(TimeSpan.FromSeconds(2)).Token);
        using var document = JsonDocument.Parse(line);
        var requestId = document.RootElement.GetProperty("requestId").GetString();
        await SendAsync(client, Result(requestId!, "unavailable", "spawn_random_enemy is registered and not implemented yet"));
        var result = await pending;
        Assert.Equal(EffectStatus.Unavailable, result.Status);
        Assert.Contains("not implemented", result.Message);
    }

    [Fact]
    public async Task EffectIsAskedForAgainWhileTheModSaysToWait()
    {
        _queue.ReadyPollInterval = TimeSpan.FromMilliseconds(10);
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: true));
        await WaitUntilAsync(() => _bridge.State.IsSafe);
        var preset = _catalog.Presets.Single(item => item.HandlerId == "adjust_souls");

        var pending = _queue.EnqueueAsync(preset.Id, "manual");
        var first = await ReadRequestIdAsync(client);
        await SendAsync(client, Result(first, "retry", "a load screen is open"));
        var second = await ReadRequestIdAsync(client);
        await SendAsync(client, Result(second, "success", "souls 140 -> 40"));

        Assert.NotEqual(first, second);
        Assert.Equal(EffectStatus.Success, (await pending).Status);
    }

    [Fact]
    public async Task EffectWaitsForACharacterThenFires()
    {
        _queue.ReadyPollInterval = TimeSpan.FromMilliseconds(10);
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: false));
        await WaitUntilAsync(() => _bridge.State is { CanWait: true, InGame: false });
        var preset = _catalog.Presets.Single(item => item.HandlerId == "adjust_souls");

        var pending = _queue.EnqueueAsync(preset.Id, "manual");
        await Task.Delay(50);
        Assert.False(pending.IsCompleted);

        await SendAsync(client, Hello(offline: true, inGame: true));
        var requestId = await ReadRequestIdAsync(client);
        await SendAsync(client, Result(requestId, "success", "souls 140 -> 40"));
        Assert.Equal(EffectStatus.Success, (await pending).Status);
    }

    [Fact]
    public async Task ASecondEffectQueuesBehindOneThatIsWaiting()
    {
        _queue.ReadyPollInterval = TimeSpan.FromMilliseconds(10);
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: false));
        await WaitUntilAsync(() => _bridge.State is { CanWait: true, InGame: false });
        var firstPreset = _catalog.Presets.Single(item => item.HandlerId == "adjust_souls");
        var secondPreset = _catalog.Presets.Single(item => item.HandlerId == "kill_player");

        var first = _queue.EnqueueAsync(firstPreset.Id, "manual");
        var second = _queue.EnqueueAsync(secondPreset.Id, "manual");
        await Task.Delay(50);
        Assert.False(first.IsCompleted);
        Assert.False(second.IsCompleted);

        await SendAsync(client, Hello(offline: true, inGame: true));
        var firstId = await ReadRequestIdAsync(client);
        await SendAsync(client, Result(firstId, "success", "souls 140 -> 40"));
        Assert.Equal(EffectStatus.Success, (await first).Status);

        var secondId = await ReadRequestIdAsync(client);
        await SendAsync(client, Result(secondId, "success", "hp 100 -> 0"));
        Assert.NotEqual(firstId, secondId);
        Assert.Equal(EffectStatus.Success, (await second).Status);
    }

    [Fact]
    public async Task EffectGivesUpIfTheGameNeverBecomesReady()
    {
        _queue.ReadyPollInterval = TimeSpan.FromMilliseconds(10);
        _queue.ReadyTimeout = TimeSpan.FromMilliseconds(100);
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: false));
        await WaitUntilAsync(() => _bridge.State is { CanWait: true, InGame: false });
        var preset = _catalog.Presets.Single(item => item.HandlerId == "adjust_souls");

        var result = await _queue.EnqueueAsync(preset.Id, "manual");

        Assert.Equal(EffectStatus.Failed, result.Status);
        Assert.Contains("gave up", result.Message);
    }

    [Fact]
    public async Task PauseHoldsEffectsAndRunsThemInOrderOnResume()
    {
        _queue.ReadyPollInterval = TimeSpan.FromMilliseconds(10);
        _queue.ReadyTimeout = TimeSpan.FromMilliseconds(50);
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: true));
        await WaitUntilAsync(() => _bridge.State.IsSafe);
        _queue.Paused = true;

        var first = _queue.EnqueueAsync(Preset("adjust_souls").Id, "twitch:Viewer");
        var second = _queue.EnqueueAsync(Preset("kill_player").Id, "manual");
        await Task.Delay(150);
        Assert.False(first.IsCompleted);
        Assert.False(second.IsCompleted);

        _queue.Paused = false;
        var firstLine = await ReadLineAsync(client, new CancellationTokenSource(TimeSpan.FromSeconds(2)).Token);
        using var document = JsonDocument.Parse(firstLine);
        Assert.Equal("adjust_souls", document.RootElement.GetProperty("handler").GetString());
        await SendAsync(client, Result(document.RootElement.GetProperty("requestId").GetString()!, "success", "ok"));
        Assert.Equal(EffectStatus.Success, (await first).Status);

        await SendAsync(client, Result(await ReadRequestIdAsync(client), "success", "ok"));
        Assert.Equal(EffectStatus.Success, (await second).Status);
    }

    [Fact]
    public async Task QueueListsEffectsWithWhoTriggeredThemAndEmpties()
    {
        _queue.ReadyPollInterval = TimeSpan.FromMilliseconds(10);
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: true));
        await WaitUntilAsync(() => _bridge.State.IsSafe);
        _queue.Paused = true;

        var first = _queue.EnqueueAsync(Preset("adjust_souls").Id, "twitch:Viewer");
        var second = _queue.EnqueueAsync(Preset("kill_player").Id, "manual");
        await WaitUntilAsync(() => _queue.Snapshot().FirstOrDefault()?.State == "paused");

        var rows = _queue.Snapshot();
        Assert.Equal(["Lose 100 souls", "Kill player"], rows.Select(row => row.Name));
        Assert.Equal(["Viewer", "Test"], rows.Select(row => row.Who));

        _queue.Clear();
        await Task.WhenAll(first, second);
        Assert.Empty(_queue.Snapshot());
    }

    [Fact]
    public async Task ClearFailsWaitingEffectsSoRedeemsAreRefunded()
    {
        _queue.ReadyPollInterval = TimeSpan.FromMilliseconds(10);
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: false));
        await WaitUntilAsync(() => _bridge.State is { CanWait: true, InGame: false });

        var first = _queue.EnqueueAsync(Preset("adjust_souls").Id, "manual");
        var second = _queue.EnqueueAsync(Preset("kill_player").Id, "manual");
        await WaitUntilAsync(() => _queue.Snapshot().Count == 2);

        Assert.Equal(2, _queue.Clear());

        foreach (var result in await Task.WhenAll(first, second))
        {
            Assert.Equal(EffectStatus.Failed, result.Status);
            Assert.Contains("cleared", result.Message);
        }
    }

    [Fact]
    public async Task ClearLeavesAnEffectTheGameAlreadyHas()
    {
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: true));
        await WaitUntilAsync(() => _bridge.State.IsSafe);

        var pending = _queue.EnqueueAsync(Preset("adjust_souls").Id, "manual");
        var requestId = await ReadRequestIdAsync(client);

        Assert.Equal(0, _queue.Clear());

        await SendAsync(client, Result(requestId, "success", "ok"));
        Assert.Equal(EffectStatus.Success, (await pending).Status);
    }

    [Fact]
    public async Task GapDelaysTheNextEffectButNotATest()
    {
        _queue.ReadyPollInterval = TimeSpan.FromMilliseconds(10);
        _queue.MinGap = TimeSpan.FromSeconds(30);
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: true));
        await WaitUntilAsync(() => _bridge.State.IsSafe);

        var first = _queue.EnqueueAsync(Preset("adjust_souls").Id, "twitch:Viewer");
        await SendAsync(client, Result(await ReadRequestIdAsync(client), "success", "ok"));
        Assert.Equal(EffectStatus.Success, (await first).Status);

        var delayed = _queue.EnqueueAsync(Preset("kill_player").Id, "twitch:Viewer");
        await WaitUntilAsync(() => _queue.Snapshot().FirstOrDefault()?.State == "gap between effects");
        _queue.MinGap = TimeSpan.Zero;
        await SendAsync(client, Result(await ReadRequestIdAsync(client), "success", "ok"));
        Assert.Equal(EffectStatus.Success, (await delayed).Status);

        _queue.MinGap = TimeSpan.FromSeconds(30);
        var test = _queue.EnqueueAsync(Preset("one_hp").Id, "manual");
        await SendAsync(client, Result(await ReadRequestIdAsync(client), "success", "ok"));
        Assert.Equal(EffectStatus.Success, (await test).Status);
    }

    private EffectPreset Preset(string handlerId) => _catalog.Presets.Single(item => item.HandlerId == handlerId);

    private async Task<string> ReadRequestIdAsync(TcpClient client)
    {
        var line = await ReadLineAsync(client, new CancellationTokenSource(TimeSpan.FromSeconds(2)).Token);
        using var document = JsonDocument.Parse(line);
        return document.RootElement.GetProperty("requestId").GetString()!;
    }

    [Fact]
    public async Task CooldownIsLeftToTwitchSoPaidTriggersAlwaysRun()
    {
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: true));
        await WaitUntilAsync(() => _bridge.State.IsSafe);
        var preset = _catalog.Presets.Single(item => item.HandlerId == "use_selected_item");
        preset.CooldownSeconds = 30;

        foreach (var source in new[] { "twitch:Viewer", "twitch:Viewer" })
        {
            var run = _queue.EnqueueAsync(preset.Id, source);
            await SendAsync(client, Result(await ReadRequestIdAsync(client), "success", "used item"));
            Assert.Equal(EffectStatus.Success, (await run).Status);
        }
    }

    private string Hello(bool offline, bool inGame, bool sessionKnown = true, bool exeMatch = true)
    {
        var handlers = JsonSerializer.Serialize(BuiltinHandlers.All);
        return JsonSerializer.Serialize(new
        {
            type = "hello",
            protocol = 1,
            secret = _secret.Secret,
            exeMatch,
            sessionKnown,
            offline,
            inGame,
            characterName = "Ash",
            handlers = JsonSerializer.Deserialize<JsonElement>(handlers)
        });
    }

    private static string Result(string requestId, string status, string message) =>
        JsonSerializer.Serialize(new { type = "result", requestId, status, message });

    private async Task<TcpClient> ConnectAsync()
    {
        var client = new TcpClient();
        await client.ConnectAsync("127.0.0.1", _bridge.Port);
        return client;
    }

    private static async Task SendAsync(TcpClient client, string line)
    {
        var bytes = Encoding.UTF8.GetBytes(line + "\n");
        await client.GetStream().WriteAsync(bytes);
    }

    private static async Task<string> ReadLineAsync(TcpClient client, CancellationToken cancellationToken)
    {
        var stream = client.GetStream();
        var buffer = new byte[1];
        using var data = new MemoryStream();
        while (true)
        {
            var read = await stream.ReadAsync(buffer, cancellationToken);
            if (read == 0)
            {
                break;
            }

            if (buffer[0] == (byte)'\n')
            {
                break;
            }

            data.WriteByte(buffer[0]);
        }

        return Encoding.UTF8.GetString(data.ToArray());
    }

    private static async Task WaitUntilAsync(Func<bool> ready)
    {
        var deadline = DateTime.UtcNow.AddSeconds(2);
        while (!ready())
        {
            if (DateTime.UtcNow > deadline)
            {
                throw new TimeoutException("the bridge did not reach the expected state");
            }

            await Task.Delay(20);
        }
    }
}
