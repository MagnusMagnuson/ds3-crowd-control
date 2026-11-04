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
        var randomWeapon = loaded.Presets.Single(preset => preset.Name == "Random Weapon" && preset.HandlerId == "random_weapon");
        Assert.Equal(0, randomWeapon.Arguments["weaponLevel"].GetInt32());
        Assert.Equal(0, randomWeapon.Arguments["infusion"].GetInt32());
        Assert.Equal(1, randomWeapon.Arguments["usableOnly"].GetInt32());
        Assert.Equal(1, randomWeapon.Arguments["removeOld"].GetInt32());
        Assert.Equal(0, randomWeapon.Arguments["includeMissingDlc"].GetInt32());
        var halfHealth = loaded.Presets.Single(preset => preset.HandlerId == "set_health_percent");
        Assert.Equal(50, halfHealth.Arguments["percent"].GetInt32());
        Assert.Contains(loaded.Presets, preset => preset.HandlerId == "set_health_random_percent");
        Assert.Contains(loaded.Presets, preset => preset.HandlerId == "spawn_npc_invader");
        var souls = loaded.Presets.Single(preset => preset.HandlerId == "adjust_souls");
        Assert.Equal(-100, souls.Arguments["delta"].GetInt32());
    }

    [Fact]
    public void ManagerSettingsRoundTrip()
    {
        var path = Path.Combine(
            Path.GetTempPath(),
            "ds3-cc-tests",
            Guid.NewGuid().ToString("N"),
            "manager-settings.json");
        ManagerSettingsStore.Save(new ManagerSettings { FocusGameForItemTest = true }, path);

        Assert.True(ManagerSettingsStore.Load(path).FocusGameForItemTest);
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
    public async Task DisabledPresetDoesNotFire()
    {
        var preset = _catalog.Presets.Single(item => item.HandlerId == "adjust_souls");
        preset.Enabled = false;
        var result = await _queue.EnqueueAsync(preset.Id, "manual");
        Assert.Equal(EffectStatus.Failed, result.Status);
        Assert.Contains("disabled", result.Message);
    }

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
        var preset = _catalog.Presets.Single(item => item.HandlerId == "spawn_npc_invader");
        var pending = _queue.EnqueueAsync(preset.Id, "manual");
        var line = await ReadLineAsync(client, new CancellationTokenSource(TimeSpan.FromSeconds(2)).Token);
        using var document = JsonDocument.Parse(line);
        var requestId = document.RootElement.GetProperty("requestId").GetString();
        await SendAsync(client, Result(requestId!, "unavailable", "spawn_npc_invader is registered and not implemented yet"));
        var result = await pending;
        Assert.Equal(EffectStatus.Unavailable, result.Status);
        Assert.Contains("not implemented", result.Message);
    }

    [Fact]
    public async Task CooldownBlocksTheNextFire()
    {
        using var client = await ConnectAsync();
        await SendAsync(client, Hello(offline: true, inGame: true));
        await WaitUntilAsync(() => _bridge.State.IsSafe);
        var preset = _catalog.Presets.Single(item => item.HandlerId == "use_selected_item");
        preset.CooldownSeconds = 30;

        var first = _queue.EnqueueAsync(preset.Id, "manual");
        var line = await ReadLineAsync(client, new CancellationTokenSource(TimeSpan.FromSeconds(2)).Token);
        using var document = JsonDocument.Parse(line);
        await SendAsync(client, Result(document.RootElement.GetProperty("requestId").GetString()!, "success", "used item"));
        Assert.Equal(EffectStatus.Success, (await first).Status);

        var second = await _queue.EnqueueAsync(preset.Id, "manual");
        Assert.Equal(EffectStatus.Failed, second.Status);
        Assert.Contains("cooldown", second.Message);
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
