using System.Text.Json;
using System.Text.Json.Nodes;
using Ds3ViewerControl.Core;
using Xunit;

namespace Ds3ViewerControl.Core.Tests;

/// <summary>A stand-in for Discord's web API that writes down what it was asked to do.</summary>
internal sealed class FakeDiscord : IDiscordRest
{
    public List<string> Calls { get; } = [];
    public List<JsonObject> Panels { get; } = [];
    public HashSet<string> GoneMessages { get; } = [];
    private int _nextMessage;

    public Task<DiscordBotUser> GetMeAsync(CancellationToken cancellationToken) =>
        Task.FromResult(new DiscordBotUser("app-1", "TestBot"));

    public Task<string> PostMessageAsync(string channelId, JsonObject message, CancellationToken cancellationToken)
    {
        var id = $"new-{++_nextMessage}";
        Record($"post {channelId} -> {id}");
        return Task.FromResult(id);
    }

    public Task<bool> EditMessageAsync(string channelId, string messageId, JsonObject message, CancellationToken cancellationToken)
    {
        Record($"edit {messageId}");
        return Task.FromResult(!GoneMessages.Contains(messageId));
    }

    public Task DeleteMessageAsync(string channelId, string messageId, CancellationToken cancellationToken)
    {
        Record($"delete {messageId}");
        return Task.CompletedTask;
    }

    public Task ReplyAsync(string interactionId, string interactionToken, JsonObject message, CancellationToken cancellationToken)
    {
        Record($"reply {interactionToken}: {Status(message)}", message);
        return Task.CompletedTask;
    }

    public Task UpdateAsync(string interactionId, string interactionToken, JsonObject message, CancellationToken cancellationToken)
    {
        Record($"update {interactionToken}: {Status(message)}", message);
        return Task.CompletedTask;
    }

    public Task EditResponseAsync(string applicationId, string interactionToken, JsonObject message, CancellationToken cancellationToken)
    {
        Record($"change {interactionToken}: {Status(message)}", message);
        return Task.CompletedTask;
    }

    public Task DeleteResponseAsync(string applicationId, string interactionToken, CancellationToken cancellationToken)
    {
        Record($"remove {interactionToken}");
        return Task.CompletedTask;
    }

    /// <summary>The first line of a personal panel: what happened with the last press.</summary>
    private static string Status(JsonObject message) => message["content"]!.GetValue<string>().Split('\n')[0];

    private void Record(string call, JsonObject? panel = null)
    {
        lock (Calls)
        {
            Calls.Add(call);
            if (panel is not null)
            {
                Panels.Add(panel);
            }
        }
    }
}

public class DiscordPanelTests
{
    private static List<EffectPreset> Presets(int count) =>
        Enumerable.Range(1, count).Select(index => new EffectPreset { Id = $"p{index}", Name = $"Effect {index}" }).ToList();

    [Fact]
    public void An_empty_panel_says_so()
    {
        var messages = DiscordPanel.Messages([], online: true);

        var message = Assert.Single(messages);
        Assert.Equal(DiscordPanel.EmptyText, message["content"]!.GetValue<string>());
        Assert.Empty(message["components"]!.AsArray());
    }

    [Fact]
    public void Buttons_fill_rows_of_five_and_messages_of_twenty_five()
    {
        var messages = DiscordPanel.Messages(Presets(27), online: true);

        Assert.Equal(2, messages.Count);
        var firstRows = messages[0]["components"]!.AsArray();
        Assert.Equal(5, firstRows.Count);
        Assert.All(firstRows, row => Assert.Equal(5, row!["components"]!.AsArray().Count));
        var secondRows = messages[1]["components"]!.AsArray();
        Assert.Equal(2, Assert.Single(secondRows)!["components"]!.AsArray().Count);
        Assert.Equal(DiscordPanel.Heading, messages[0]["content"]!.GetValue<string>());
        Assert.Equal(DiscordPanel.MoreHeading, messages[1]["content"]!.GetValue<string>());
    }

    [Fact]
    public void Each_button_carries_its_preset_and_name()
    {
        var button = DiscordPanel.Messages(Presets(1), online: true)[0]["components"]![0]!["components"]![0]!;

        Assert.Equal("Effect 1", button["label"]!.GetValue<string>());
        Assert.Equal("p1", DiscordPanel.PresetIdFrom(button["custom_id"]!.GetValue<string>()));
        Assert.False(button["disabled"]!.GetValue<bool>());
    }

    [Fact]
    public void An_offline_panel_greys_out_its_buttons()
    {
        var message = DiscordPanel.Messages(Presets(3), online: false)[0];

        Assert.Equal(DiscordPanel.OfflineText, message["content"]!.GetValue<string>());
        Assert.All(message["components"]![0]!["components"]!.AsArray(), button => Assert.True(button!["disabled"]!.GetValue<bool>()));
    }

    [Fact]
    public void Long_names_are_cut_to_what_Discord_allows()
    {
        var presets = new List<EffectPreset> { new() { Id = "long", Name = new string('x', 120) } };

        var label = DiscordPanel.Messages(presets, online: true)[0]["components"]![0]!["components"]![0]!["label"]!.GetValue<string>();

        Assert.Equal(80, label.Length);
    }

    [Fact]
    public void A_button_on_cooldown_is_grey_with_a_live_countdown()
    {
        var ready = new DateTimeOffset(2026, 11, 15, 12, 2, 0, TimeSpan.Zero);
        var message = DiscordPanel.Messages(Presets(2), online: true, new Dictionary<string, DateTimeOffset> { ["p1"] = ready })[0];

        var buttons = message["components"]![0]!["components"]!.AsArray();
        Assert.True(buttons[0]!["disabled"]!.GetValue<bool>());
        Assert.False(buttons[1]!["disabled"]!.GetValue<bool>());
        Assert.Contains($"- Effect 1: ready <t:{ready.ToUnixTimeSeconds()}:R>", message["content"]!.GetValue<string>());
        Assert.DoesNotContain("Effect 2", message["content"]!.GetValue<string>());
    }

    [Theory]
    [InlineData("ds3cc:abc", "abc")]
    [InlineData("ds3cc:", null)]
    [InlineData("ds3cc:me:abc", null)]
    [InlineData("other:abc", null)]
    [InlineData(null, null)]
    public void Only_our_own_buttons_are_recognised(string? customId, string? expected) =>
        Assert.Equal(expected, DiscordPanel.PresetIdFrom(customId));

    [Fact]
    public async Task Updating_edits_the_old_messages_and_removes_extra_ones()
    {
        var (connection, discord) = await ConnectedAsync();

        var ids = await connection.UpdatePanelAsync("chan", ["old-1", "old-2"], Presets(3), online: true, default);

        Assert.Equal(["old-1"], ids);
        Assert.Equal(["edit old-1", "delete old-2"], discord.Calls);
    }

    [Fact]
    public async Task A_deleted_message_is_posted_again_and_the_rest_reposted_in_order()
    {
        var (connection, discord) = await ConnectedAsync();
        discord.GoneMessages.Add("old-1");

        var ids = await connection.UpdatePanelAsync("chan", ["old-1", "old-2"], Presets(30), online: true, default);

        Assert.Equal(["new-1", "new-2"], ids);
        Assert.Equal(["edit old-1", "delete old-1", "delete old-2", "post chan -> new-1", "post chan -> new-2"], discord.Calls);
    }

    private static async Task<(DiscordConnection, FakeDiscord)> ConnectedAsync()
    {
        var discord = new FakeDiscord();
        var folder = Path.Combine(Path.GetTempPath(), "ds3cc-tests", Guid.NewGuid().ToString("N"));
        var store = new DiscordTokenStore(Path.Combine(folder, "discord-bot.bin"));
        store.Save("token");
        var source = new DiscordTriggerSource(new Catalog(), _ => { });
        var connection = new DiscordConnection(source, store, new HttpClient(), _ => { })
        {
            RestFactory = _ => discord,
            GatewayRunner = (_, _, ready, cancellationToken) =>
            {
                ready("app-1");
                return Task.Delay(Timeout.Infinite, cancellationToken);
            }
        };
        await connection.ConnectAsync(default);
        return (connection, discord);
    }
}

public class DiscordPressTests
{
    private readonly Catalog _catalog = Catalog.CreateDefault();
    private readonly FakeDiscord _discord = new();
    private readonly List<string> _log = [];
    private readonly List<string> _ran = [];
    private readonly List<(TimeSpan Wait, TaskCompletionSource Done)> _delays = [];
    private readonly DiscordTriggerSource _source;
    private readonly EffectPreset _first;
    private readonly EffectPreset _second;
    private DateTimeOffset _now = new(2026, 11, 15, 12, 0, 0, TimeSpan.Zero);
    private EffectStatus _outcome = EffectStatus.Success;

    public DiscordPressTests()
    {
        _first = _catalog.Presets[0];
        _second = _catalog.Presets[1];
        foreach (var preset in new[] { _first, _second })
        {
            preset.DiscordEnabled = true;
            preset.DiscordCooldownSeconds = 120;
        }

        _source = new DiscordTriggerSource(_catalog, _log.Add)
        {
            Rest = _discord,
            ApplicationId = "app-1",
            ViewerCooldownSeconds = 60,
            Now = () => _now,
            Delay = (wait, _) =>
            {
                var done = new TaskCompletionSource();
                lock (_delays)
                {
                    _delays.Add((wait, done));
                }

                return done.Task;
            },
            RunEffect = (presetId, source, _) =>
            {
                _ran.Add($"{presetId} {source}");
                return Task.FromResult(new EffectResult("", _outcome, _outcome == EffectStatus.Success ? "ok" : "no player"));
            }
        };
    }

    private static JsonElement Press(string presetId, string userId = "u1", string? nick = "Nick", string? globalName = "Global",
        string username = "user1", int type = 3, string token = "token-1", bool personal = false) =>
        Button((personal ? DiscordPanel.PersonalPrefix : DiscordPanel.ButtonPrefix) + presetId, userId, nick, globalName, username, type, token);

    private static JsonElement Button(string customId, string userId = "u1", string? nick = "Nick", string? globalName = "Global",
        string username = "user1", int type = 3, string token = "token-1") =>
        JsonSerializer.SerializeToElement(new
        {
            id = "interaction-1",
            token,
            type,
            data = new { custom_id = customId },
            member = new { nick, user = new { id = userId, global_name = globalName, username } }
        });

    /// <summary>The refresh or removal the source scheduled last, once it has.</summary>
    private async Task<(TimeSpan Wait, TaskCompletionSource Done)> NextDelayAsync(int count)
    {
        for (var tries = 0; tries < 200; tries++)
        {
            lock (_delays)
            {
                if (_delays.Count >= count)
                {
                    return _delays[count - 1];
                }
            }

            await Task.Delay(10);
        }

        throw new TimeoutException("nothing was scheduled");
    }

    private async Task WaitForCallAsync(string call)
    {
        for (var tries = 0; tries < 200; tries++)
        {
            lock (_discord.Calls)
            {
                if (_discord.Calls.Contains(call))
                {
                    return;
                }
            }

            await Task.Delay(10);
        }

        throw new TimeoutException($"never saw {call}");
    }

    private static List<bool> Disabled(JsonObject panel) =>
        panel["components"]!.AsArray()
            .SelectMany(row => row!["components"]!.AsArray())
            .Select(button => button!["disabled"]!.GetValue<bool>())
            .ToList();

    [Fact]
    public async Task A_press_queues_the_effect_and_opens_a_personal_panel()
    {
        await _source.HandleAsync(Press(_first.Id), default);

        Assert.Equal([$"{_first.Id} discord:Nick"], _ran);
        Assert.Equal([$"reply token-1: {_first.Name} is queued.", $"change token-1: {_first.Name} ran."], _discord.Calls);
        var panel = _discord.Panels[0];
        Assert.Contains($"You can press again {DiscordPanel.Countdown(_now + TimeSpan.FromSeconds(60))}.", panel["content"]!.GetValue<string>());
        // The viewer's own cooldown greys out every button on their panel.
        Assert.All(Disabled(panel), Assert.True);
    }

    [Fact]
    public async Task Everyone_waits_for_the_event_cooldown()
    {
        await _source.HandleAsync(Press(_first.Id, "u1"), default);
        _discord.Calls.Clear();

        await _source.HandleAsync(Press(_first.Id, "u2", token: "token-2"), default);

        Assert.Single(_ran);
        Assert.Equal([$"reply token-2: {_first.Name} is ready again in 120 seconds."], _discord.Calls);
        // Another viewer sees only the event's own button grey.
        var panel = _discord.Panels[^1];
        Assert.Equal([true, false], Disabled(panel).Take(2));
    }

    [Fact]
    public async Task A_viewer_waits_before_pressing_another_button()
    {
        await _source.HandleAsync(Press(_first.Id, "u1", token: "first"), default);
        _now += TimeSpan.FromSeconds(59);

        await _source.HandleAsync(Press(_second.Id, "u1", token: "second", personal: true), default);

        Assert.Single(_ran);
        Assert.Contains("update second: You can send another effect in 1 second.", _discord.Calls);
    }

    [Fact]
    public async Task Cooldowns_run_out()
    {
        await _source.HandleAsync(Press(_first.Id, "u1", token: "first"), default);
        _now += TimeSpan.FromSeconds(120);

        await _source.HandleAsync(Press(_first.Id, "u1", token: "second", personal: true), default);

        Assert.Equal(2, _ran.Count);
    }

    [Fact]
    public async Task A_failed_effect_gives_both_cooldowns_back()
    {
        _outcome = EffectStatus.Unavailable;
        await _source.HandleAsync(Press(_first.Id, "u1", token: "first"), default);
        Assert.Contains($"change first: {_first.Name} did not run: no player. Your cooldown was given back.", _discord.Calls);
        Assert.All(Disabled(_discord.Panels[^1]), Assert.False);
        _outcome = EffectStatus.Success;

        await _source.HandleAsync(Press(_first.Id, "u1", token: "second", personal: true), default);

        Assert.Equal(2, _ran.Count);
    }

    [Fact]
    public async Task A_press_on_the_personal_panel_changes_that_panel()
    {
        await _source.HandleAsync(Press(_first.Id, token: "first"), default);
        _now += TimeSpan.FromSeconds(61);
        _discord.Calls.Clear();

        await _source.HandleAsync(Press(_second.Id, token: "second", personal: true), default);

        Assert.Equal([$"update second: {_second.Name} is queued.", $"change second: {_second.Name} ran."], _discord.Calls);
    }

    [Fact]
    public async Task A_new_press_on_the_public_panel_replaces_the_old_personal_panel()
    {
        await _source.HandleAsync(Press(_first.Id, token: "first"), default);
        _discord.Calls.Clear();

        await _source.HandleAsync(Press(_first.Id, token: "second"), default);

        Assert.Contains("remove first", _discord.Calls);
    }

    [Fact]
    public async Task A_panel_older_than_Discord_allows_is_left_alone()
    {
        await _source.HandleAsync(Press(_first.Id, token: "first"), default);
        _discord.Calls.Clear();
        _now += TimeSpan.FromMinutes(20);

        await _source.HandleAsync(Press(_first.Id, token: "second"), default);

        Assert.DoesNotContain("remove first", _discord.Calls);
    }

    [Fact]
    public async Task The_panel_comes_back_to_life_when_the_viewer_cooldown_ends()
    {
        await _source.HandleAsync(Press(_first.Id, token: "first"), default);
        // Scheduled after "queued" and again after "ran"; the later one is current.
        var (wait, done) = await NextDelayAsync(2);
        Assert.Equal(TimeSpan.FromSeconds(60.5), wait);
        _now += TimeSpan.FromSeconds(61);
        _discord.Calls.Clear();

        done.SetResult();
        await WaitForCallAsync($"change first: {_first.Name} ran.");

        var panel = _discord.Panels[^1];
        Assert.DoesNotContain("You can press again", panel["content"]!.GetValue<string>());
        // The event's own cooldown still runs, the other button is pressable again.
        Assert.Equal([true, false], Disabled(panel).Take(2));
    }

    [Fact]
    public async Task An_unused_panel_is_removed_before_it_freezes()
    {
        _source.ViewerCooldownSeconds = 0;
        _first.DiscordCooldownSeconds = 0;
        await _source.HandleAsync(Press(_first.Id, token: "first"), default);
        var (wait, done) = await NextDelayAsync(2);
        Assert.Equal(DiscordTriggerSource.PanelLifetime, wait);

        done.SetResult();

        await WaitForCallAsync("remove first");
    }

    [Fact]
    public async Task Many_events_get_pages_on_the_personal_panel()
    {
        foreach (var preset in _catalog.Presets.Take(30))
        {
            preset.DiscordEnabled = true;
        }

        await _source.HandleAsync(Press(_first.Id, token: "first"), default);
        Assert.Contains("Page 1 of", _discord.Panels[0].ToJsonString());

        await _source.HandleAsync(Button(DiscordPanel.PagePrefix + "1", token: "second"), default);

        Assert.Contains("Page 2 of", _discord.Panels[^1].ToJsonString());
        Assert.Contains(_discord.Calls, call => call.StartsWith("update second:", StringComparison.Ordinal));
        Assert.Single(_ran);
    }

    [Fact]
    public async Task Nothing_runs_while_paused()
    {
        _source.Paused = true;

        await _source.HandleAsync(Press(_first.Id), default);

        Assert.Empty(_ran);
        Assert.Equal(["reply token-1: Effects are paused right now. Try again later."], _discord.Calls);
    }

    [Fact]
    public async Task An_event_taken_off_the_panel_does_not_run()
    {
        _first.DiscordEnabled = false;

        await _source.HandleAsync(Press(_first.Id), default);

        Assert.Empty(_ran);
        Assert.Equal(["reply token-1: That effect is not on the panel anymore."], _discord.Calls);
    }

    [Fact]
    public async Task Other_interactions_are_left_alone()
    {
        await _source.HandleAsync(Press(_first.Id, type: 2), default);

        Assert.Empty(_ran);
        Assert.Empty(_discord.Calls);
    }

    [Fact]
    public async Task The_shown_name_falls_back_from_nickname_to_display_name_to_user_name()
    {
        await _source.HandleAsync(Press(_first.Id, "u1", nick: null), default);
        await _source.HandleAsync(Press(_second.Id, "u2", nick: null, globalName: null), default);

        Assert.Equal([$"{_first.Id} discord:Global", $"{_second.Id} discord:user1"], _ran);
    }

    [Fact]
    public async Task The_public_panel_learns_which_events_are_on_cooldown()
    {
        var changes = 0;
        _source.CooldownsChanged += () => changes++;

        await _source.HandleAsync(Press(_first.Id), default);

        Assert.Equal(1, changes);
        Assert.Equal(_now + TimeSpan.FromSeconds(120), Assert.Single(_source.EventCooldowns()).Value);
        _now += TimeSpan.FromSeconds(120);
        Assert.Empty(_source.EventCooldowns());
    }

    [Fact]
    public void The_queue_shows_a_Discord_viewer_by_name() =>
        Assert.Equal("Ann", EffectQueue.Who("discord:Ann"));
}

public sealed class DiscordGatewayTests : IDisposable
{
    private readonly System.Net.HttpListener _server = new();
    private readonly Uri _url;
    private readonly CancellationTokenSource _stop = new(TimeSpan.FromSeconds(20));

    public DiscordGatewayTests()
    {
        var probe = new System.Net.Sockets.TcpListener(System.Net.IPAddress.Loopback, 0);
        probe.Start();
        var port = ((System.Net.IPEndPoint)probe.LocalEndpoint).Port;
        probe.Stop();
        _server.Prefixes.Add($"http://localhost:{port}/");
        _server.Start();
        _url = new Uri($"ws://localhost:{port}/gateway");
    }

    public void Dispose()
    {
        _stop.Cancel();
        _server.Close();
    }

    private async Task<System.Net.WebSockets.WebSocket> AcceptAsync()
    {
        var context = await _server.GetContextAsync().WaitAsync(_stop.Token);
        return (await context.AcceptWebSocketAsync(null)).WebSocket;
    }

    private Task SendAsync(System.Net.WebSockets.WebSocket socket, string text) =>
        socket.SendAsync(System.Text.Encoding.UTF8.GetBytes(text), System.Net.WebSockets.WebSocketMessageType.Text, true, _stop.Token);

    private async Task<JsonElement> ReceiveAsync(System.Net.WebSockets.WebSocket socket)
    {
        var buffer = new byte[8192];
        var result = await socket.ReceiveAsync(buffer, _stop.Token);
        return JsonDocument.Parse(System.Text.Encoding.UTF8.GetString(buffer, 0, result.Count)).RootElement.Clone();
    }

    [Fact]
    public async Task It_signs_in_and_passes_button_presses_on()
    {
        var client = new DiscordGatewayClient("secret-token", _ => { }) { Url = _url };
        var ready = new TaskCompletionSource<string>();
        var pressed = new TaskCompletionSource<string>();
        client.Ready += application => ready.TrySetResult(application);
        client.Interaction += interaction => pressed.TrySetResult(interaction.GetProperty("id").GetString()!);
        var running = client.RunAsync(_stop.Token);

        var socket = await AcceptAsync();
        await SendAsync(socket, """{"op":10,"d":{"heartbeat_interval":60000}}""");
        var identify = await ReceiveAsync(socket);
        Assert.Equal(2, identify.GetProperty("op").GetInt32());
        Assert.Equal("secret-token", identify.GetProperty("d").GetProperty("token").GetString());
        Assert.Equal(0, identify.GetProperty("d").GetProperty("intents").GetInt32());

        await SendAsync(socket, """{"op":0,"s":1,"t":"READY","d":{"user":{"id":"bot-1"},"application":{"id":"app-7"}}}""");
        Assert.Equal("app-7", await ready.Task.WaitAsync(_stop.Token));

        await SendAsync(socket, """{"op":0,"s":2,"t":"INTERACTION_CREATE","d":{"id":"press-1","type":3}}""");
        Assert.Equal("press-1", await pressed.Task.WaitAsync(_stop.Token));

        // Asked for a heartbeat, it sends one with the last number it saw.
        await SendAsync(socket, """{"op":1,"d":null}""");
        var beat = await ReceiveAsync(socket);
        Assert.Equal(1, beat.GetProperty("op").GetInt32());
        Assert.Equal(2, beat.GetProperty("d").GetInt64());

        _stop.Cancel();
        await running;
    }

    [Fact]
    public async Task A_refused_token_stops_instead_of_retrying()
    {
        var client = new DiscordGatewayClient("bad-token", _ => { }) { Url = _url, FirstRetryDelay = TimeSpan.FromMilliseconds(10) };
        var running = client.RunAsync(_stop.Token);

        var socket = await AcceptAsync();
        await SendAsync(socket, """{"op":10,"d":{"heartbeat_interval":60000}}""");
        await ReceiveAsync(socket);
        await socket.CloseOutputAsync((System.Net.WebSockets.WebSocketCloseStatus)4004, "Authentication failed.", _stop.Token);

        var error = await Assert.ThrowsAsync<DiscordException>(() => running.WaitAsync(_stop.Token));
        Assert.Contains("refused the bot token", error.Message);
    }
}
