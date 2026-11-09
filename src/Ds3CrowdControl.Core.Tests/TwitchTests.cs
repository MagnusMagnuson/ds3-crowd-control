using System.Net;
using System.Net.Sockets;
using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using Ds3CrowdControl.Core;
using Xunit;

namespace Ds3CrowdControl.Core.Tests;

public class TwitchRedemptionTests
{
    private readonly Catalog _catalog = Catalog.CreateDefault();
    private readonly FakeRewards _rewards = new();
    private readonly List<string> _log = [];
    private readonly EffectPreset _preset;
    private readonly TwitchTriggerSource _source;

    public TwitchRedemptionTests()
    {
        _preset = _catalog.Presets[0];
        _preset.RedeemEnabled = true;
        _preset.TwitchRewardId = "reward-1";
        _source = new TwitchTriggerSource(_catalog, _log.Add) { Rewards = _rewards };
    }

    private static JsonElement Redemption(string rewardId = "reward-1", string status = "unfulfilled") =>
        JsonDocument.Parse($$$$"""
            {"id":"redeem-1","user_name":"Viewer","status":"{{{{status}}}}","reward":{"id":"{{{{rewardId}}}}"}}
            """).RootElement;

    [Fact]
    public async Task A_working_effect_completes_the_redemption()
    {
        _source.RunEffect = (_, _, _) => Task.FromResult(new EffectResult("", EffectStatus.Success, "ok"));

        await _source.HandleRedemptionAsync(Redemption(), default);

        Assert.Equal(["reward-1/redeem-1=FULFILLED"], _rewards.Statuses);
    }

    [Theory]
    [InlineData(EffectStatus.Failed)]
    [InlineData(EffectStatus.Unavailable)]
    [InlineData(EffectStatus.Retry)]
    public async Task A_failed_effect_refunds_the_points(EffectStatus status)
    {
        _source.RunEffect = (_, _, _) => Task.FromResult(new EffectResult("", status, "no"));

        await _source.HandleRedemptionAsync(Redemption(), default);

        Assert.Equal(["reward-1/redeem-1=CANCELED"], _rewards.Statuses);
    }

    [Fact]
    public async Task An_effect_that_throws_refunds_the_points()
    {
        _source.RunEffect = (_, _, _) => throw new InvalidOperationException("broken");

        await _source.HandleRedemptionAsync(Redemption(), default);

        Assert.Equal(["reward-1/redeem-1=CANCELED"], _rewards.Statuses);
    }

    [Fact]
    public async Task The_viewer_name_reaches_the_queue()
    {
        string? source = null;
        _source.RunEffect = (_, from, _) =>
        {
            source = from;
            return Task.FromResult(new EffectResult("", EffectStatus.Success, "ok"));
        };

        await _source.HandleRedemptionAsync(Redemption(), default);

        Assert.Equal("twitch:Viewer", source);
    }

    [Fact]
    public async Task Rewards_this_app_does_not_know_are_ignored()
    {
        var ran = false;
        _source.RunEffect = (_, _, _) =>
        {
            ran = true;
            return Task.FromResult(new EffectResult("", EffectStatus.Success, "ok"));
        };

        await _source.HandleRedemptionAsync(Redemption("someone-elses-reward"), default);

        Assert.False(ran);
        Assert.Empty(_rewards.Statuses);
    }

    [Fact]
    public async Task Redemptions_already_handled_are_ignored()
    {
        var ran = false;
        _source.RunEffect = (_, _, _) =>
        {
            ran = true;
            return Task.FromResult(new EffectResult("", EffectStatus.Success, "ok"));
        };

        await _source.HandleRedemptionAsync(Redemption(status: "fulfilled"), default);

        Assert.False(ran);
    }

    [Fact]
    public async Task A_rush_past_the_limit_is_refunded()
    {
        _source.WaitingLimit = 3;
        var release = new TaskCompletionSource<EffectResult>();
        _source.RunEffect = (_, _, _) => release.Task;
        var waiting = Enumerable.Range(0, _source.WaitingLimit)
            .Select(_ => _source.HandleRedemptionAsync(Redemption(), default))
            .ToList();

        await _source.HandleRedemptionAsync(Redemption(), default);

        Assert.Equal(["reward-1/redeem-1=CANCELED"], _rewards.Statuses);
        release.SetResult(new EffectResult("", EffectStatus.Success, "ok"));
        await Task.WhenAll(waiting);
        Assert.Equal(_source.WaitingLimit, _rewards.Statuses.Count(status => status.EndsWith("FULFILLED")));
    }

    [Fact]
    public async Task No_limit_never_refunds_a_rush()
    {
        var release = new TaskCompletionSource<EffectResult>();
        _source.RunEffect = (_, _, _) => release.Task;
        var waiting = Enumerable.Range(0, 50)
            .Select(_ => _source.HandleRedemptionAsync(Redemption(), default))
            .ToList();

        Assert.Empty(_rewards.Statuses);
        release.SetResult(new EffectResult("", EffectStatus.Success, "ok"));
        await Task.WhenAll(waiting);
        Assert.Equal(50, _rewards.Statuses.Count(status => status.EndsWith("FULFILLED")));
    }

    [Fact]
    public async Task Syncing_during_a_pause_hides_the_rewards()
    {
        _rewards.Existing.Add(new TwitchReward("reward-1", _preset.Name, _preset.RedeemCost, true));
        _source.Paused = true;

        await _source.SyncRewardsAsync(default);

        Assert.False(_rewards.Updated.Single().Settings.Enabled);
    }

    [Fact]
    public async Task Syncing_skips_a_reward_that_already_matches()
    {
        _rewards.Existing.Add(new TwitchReward("reward-1", _preset.Name, _preset.RedeemCost, true, _preset.CooldownSeconds));

        await _source.SyncRewardsAsync(default);

        Assert.Empty(_rewards.Updated);
    }

    [Fact]
    public async Task A_burst_of_requests_runs_once()
    {
        var runs = 0;
        var debouncer = new Debouncer(TimeSpan.FromMilliseconds(60), () =>
        {
            Interlocked.Increment(ref runs);
            return Task.CompletedTask;
        });

        for (var request = 0; request < 5; request++)
        {
            debouncer.Request();
            await Task.Delay(5);
        }

        await Task.Delay(200);
        Assert.Equal(1, runs);
    }

    [Fact]
    public async Task A_cancelled_request_does_not_run()
    {
        var runs = 0;
        var debouncer = new Debouncer(TimeSpan.FromMilliseconds(30), () =>
        {
            Interlocked.Increment(ref runs);
            return Task.CompletedTask;
        });

        debouncer.Request();
        debouncer.Cancel();

        await Task.Delay(100);
        Assert.Equal(0, runs);
    }

    [Fact]
    public async Task Syncing_creates_a_reward_and_keeps_its_id()
    {
        _preset.TwitchRewardId = null;
        _preset.RedeemCost = 750;

        var changed = await _source.SyncRewardsAsync(default);

        Assert.True(changed);
        Assert.Equal("created-1", _preset.TwitchRewardId);
        Assert.Equal(750, _rewards.Created.Single().Cost);
    }

    [Fact]
    public async Task Syncing_updates_a_reward_instead_of_making_a_second_one()
    {
        _rewards.Existing.Add(new TwitchReward("reward-1", "old name"));

        await _source.SyncRewardsAsync(default);

        Assert.Empty(_rewards.Created);
        Assert.Equal("reward-1", _rewards.Updated.Single().Id);
        Assert.Equal(_preset.Name, _rewards.Updated.Single().Settings.Title);
    }

    [Fact]
    public async Task Syncing_finds_a_reward_by_title_when_the_id_was_lost()
    {
        _preset.TwitchRewardId = null;
        _rewards.Existing.Add(new TwitchReward("found", _preset.Name));

        await _source.SyncRewardsAsync(default);

        Assert.Equal("found", _preset.TwitchRewardId);
        Assert.Empty(_rewards.Created);
    }

    [Fact]
    public async Task Turning_a_reward_off_hides_it_and_keeps_it()
    {
        _rewards.Existing.Add(new TwitchReward("reward-1", _preset.Name, _preset.RedeemCost, true));
        _preset.RedeemEnabled = false;

        await _source.SyncRewardsAsync(default);

        Assert.Equal("reward-1", _preset.TwitchRewardId);
        Assert.False(_rewards.Updated.Single().Settings.Enabled);
        Assert.Empty(_rewards.Created);
    }

    [Fact]
    public async Task Turning_a_reward_back_on_reuses_the_same_reward()
    {
        _rewards.Existing.Add(new TwitchReward("reward-1", _preset.Name, _preset.RedeemCost, false));

        await _source.SyncRewardsAsync(default);

        Assert.Equal("reward-1", _rewards.Updated.Single().Id);
        Assert.True(_rewards.Updated.Single().Settings.Enabled);
        Assert.Empty(_rewards.Created);
    }

    [Fact]
    public async Task A_reward_with_no_preset_is_hidden_not_removed()
    {
        _preset.RedeemEnabled = false;
        _rewards.Existing.Add(new TwitchReward("old", "Gone", 100, true, 30));

        await _source.SyncRewardsAsync(default);

        var hidden = _rewards.Updated.Single();
        Assert.Equal("old", hidden.Id);
        Assert.Equal(new TwitchRewardSettings("Gone", 100, false, 30), hidden.Settings);
    }

    [Fact]
    public async Task A_redeem_turned_off_a_moment_ago_is_refunded()
    {
        var ran = false;
        _preset.RedeemEnabled = false;
        _source.RunEffect = (_, _, _) =>
        {
            ran = true;
            return Task.FromResult(new EffectResult("", EffectStatus.Success, "ok"));
        };

        await _source.HandleRedemptionAsync(Redemption(), default);

        Assert.False(ran);
        Assert.Equal(["reward-1/redeem-1=CANCELED"], _rewards.Statuses);
    }

    private static JsonElement Cheer(int bits, string? viewer = "Viewer") =>
        JsonSerializer.SerializeToElement(new { bits, user_name = viewer });

    private static JsonElement Gift(int total, bool anonymous = false) =>
        JsonSerializer.SerializeToElement(new { total, is_anonymous = anonymous, user_name = anonymous ? null : "Gifter" });

    private List<(string Preset, string Source)> RecordRuns()
    {
        var runs = new List<(string, string)>();
        _source.RunEffect = (preset, source, _) =>
        {
            runs.Add((preset, source));
            return Task.FromResult(new EffectResult("", EffectStatus.Success, "ok"));
        };
        return runs;
    }

    [Fact]
    public async Task Exactly_the_set_bits_run_the_event()
    {
        _preset.BitsEnabled = true;
        _preset.BitsAmount = 250;
        var runs = RecordRuns();

        await _source.HandleBitsAsync(Cheer(249), default);
        await _source.HandleBitsAsync(Cheer(251), default);
        Assert.Empty(runs);

        await _source.HandleBitsAsync(Cheer(250), default);
        Assert.Equal([(_preset.Id, "twitch:Viewer")], runs);
    }

    [Fact]
    public async Task Bits_are_ignored_while_the_event_has_bits_off()
    {
        _preset.BitsAmount = 250;
        var runs = RecordRuns();

        await _source.HandleBitsAsync(Cheer(250), default);

        Assert.Empty(runs);
    }

    [Fact]
    public async Task Events_sharing_an_amount_run_only_one()
    {
        foreach (var preset in _catalog.Presets.Take(3))
        {
            preset.BitsEnabled = true;
            preset.BitsAmount = 100;
        }

        var runs = RecordRuns();

        await _source.HandleBitsAsync(Cheer(100), default);

        Assert.Contains(_catalog.Presets.Take(3), preset => preset.Id == runs.Single().Preset);
    }

    [Fact]
    public async Task A_gift_of_the_set_count_runs_the_event()
    {
        _preset.GiftEnabled = true;
        _preset.GiftAmount = 5;
        var runs = RecordRuns();

        await _source.HandleGiftAsync(Gift(4), default);
        Assert.Empty(runs);

        await _source.HandleGiftAsync(Gift(5), default);
        Assert.Equal([(_preset.Id, "twitch:Gifter")], runs);
    }

    [Fact]
    public async Task An_anonymous_gift_shows_as_anonymous()
    {
        _preset.GiftEnabled = true;
        _preset.GiftAmount = 5;
        var runs = RecordRuns();

        await _source.HandleGiftAsync(Gift(5, anonymous: true), default);

        Assert.Equal("twitch:Anonymous", runs.Single().Source);
    }

    [Fact]
    public async Task Failed_bits_are_logged_because_twitch_cannot_refund_them()
    {
        _preset.BitsEnabled = true;
        _preset.BitsAmount = 100;
        _source.RunEffect = (_, _, _) => Task.FromResult(new EffectResult("", EffectStatus.Failed, "game closed"));

        await _source.HandleBitsAsync(Cheer(100), default);

        Assert.Contains(_log, line => line.Contains("100 Bits") && line.Contains("cannot refund"));
        Assert.Empty(_rewards.Statuses);
    }

    private static JsonElement TestRedemption(string title, string status) =>
        JsonSerializer.SerializeToElement(new
        {
            id = "redeem-1",
            user_name = "Viewer",
            status,
            reward = new { id = "made-up-by-the-cli", title }
        });

    [Fact]
    public async Task The_test_server_finds_a_redeem_by_title_whatever_its_status()
    {
        _source.TestMode = true;
        var runs = RecordRuns();

        await _source.HandleRedemptionAsync(TestRedemption(_preset.Name.ToUpperInvariant(), "fulfilled"), default);

        Assert.Equal([(_preset.Id, "twitch:Viewer")], runs);
    }

    [Fact]
    public async Task A_title_alone_means_nothing_outside_the_test_server()
    {
        var runs = RecordRuns();

        await _source.HandleRedemptionAsync(TestRedemption(_preset.Name, "unfulfilled"), default);

        Assert.Empty(runs);
    }

    [Fact]
    public void Every_trigger_type_gets_subscribed()
    {
        Assert.Equal(
            ["channel.channel_points_custom_reward_redemption.add", "channel.bits.use", "channel.subscription.gift"],
            TwitchTriggerSource.EventTypes);
    }

    [Fact]
    public async Task A_reward_that_fails_to_update_keeps_its_id()
    {
        _rewards.Existing.Add(new TwitchReward("reward-1", _preset.Name));
        _rewards.UpdateError = new TwitchException("bad title", HttpStatusCode.BadRequest);

        await _source.SyncRewardsAsync(default);

        Assert.Equal("reward-1", _preset.TwitchRewardId);
    }

    [Fact]
    public async Task A_channel_without_channel_points_stops_the_sync()
    {
        _preset.TwitchRewardId = null;
        _rewards.Existing.Add(new TwitchReward("unrelated", "Other"));
        _rewards.CreateError = new TwitchException("not affiliate", HttpStatusCode.Forbidden);

        await _source.SyncRewardsAsync(default);

        Assert.Contains(_log, line => line.Contains("Affiliate"));
    }

    [Fact]
    public async Task Pausing_disables_live_rewards()
    {
        await _source.PauseRewardsAsync(default);

        Assert.False(_rewards.Updated.Single().Settings.Enabled);
    }

    [Fact]
    public void Long_preset_names_fit_the_title_limit()
    {
        var title = TwitchTriggerSource.RewardTitle("", new string('x', 80));

        Assert.Equal(45, title.Length);
    }

    [Fact]
    public void A_long_name_loses_its_end_but_keeps_the_prefix()
    {
        var title = TwitchTriggerSource.RewardTitle("DS3: ", new string('x', 80));

        Assert.Equal(45, title.Length);
        Assert.StartsWith("DS3: x", title);
    }

    [Fact]
    public async Task The_prefix_goes_in_front_of_new_redeems()
    {
        _preset.TwitchRewardId = null;
        _source.RewardPrefix = "DS3: ";

        await _source.SyncRewardsAsync(default);

        Assert.Equal("DS3: " + _preset.Name, _rewards.Created.Single().Title);
    }

    [Fact]
    public async Task Changing_the_prefix_renames_the_redeem_already_made()
    {
        _rewards.Existing.Add(new TwitchReward("reward-1", _preset.Name, _preset.RedeemCost, true, _preset.CooldownSeconds));
        _source.RewardPrefix = "DS3: ";

        await _source.SyncRewardsAsync(default);

        Assert.Empty(_rewards.Created);
        Assert.Equal(("reward-1", "DS3: " + _preset.Name), (_rewards.Updated.Single().Id, _rewards.Updated.Single().Settings.Title));
    }

    [Fact]
    public async Task A_name_taken_by_another_tool_points_to_the_prefix()
    {
        _preset.TwitchRewardId = null;
        _rewards.CreateError = new TwitchException("CREATE_CUSTOM_REWARD_DUPLICATE_REWARD", HttpStatusCode.BadRequest);

        await _source.SyncRewardsAsync(default);

        Assert.Contains(_log, line => line.Contains("redeem name prefix"));
    }

    [Fact]
    public async Task A_test_redeem_is_found_by_its_prefixed_title()
    {
        _source.TestMode = true;
        _source.RewardPrefix = "DS3: ";
        _preset.TwitchRewardId = null;
        _source.RunEffect = (_, _, _) => Task.FromResult(new EffectResult("", EffectStatus.Success, "ok"));
        var redemption = JsonDocument.Parse($$$"""
            {"id":"redeem-1","user_name":"Viewer","status":"unfulfilled","reward":{"id":"cli","title":"DS3: {{{_preset.Name}}}"}}
            """).RootElement;

        await _source.HandleRedemptionAsync(redemption, default);

        Assert.Equal(["cli/redeem-1=FULFILLED"], _rewards.Statuses);
    }

    private sealed class FakeRewards : ITwitchRewards
    {
        public List<TwitchReward> Existing { get; } = [];
        public List<TwitchRewardSettings> Created { get; } = [];
        public List<(string Id, TwitchRewardSettings Settings)> Updated { get; } = [];
        public List<string> Statuses { get; } = [];
        public TwitchException? CreateError { get; set; }
        public TwitchException? UpdateError { get; set; }

        public Task<IReadOnlyList<TwitchReward>> GetManageableRewardsAsync(CancellationToken cancellationToken) =>
            Task.FromResult<IReadOnlyList<TwitchReward>>(Existing);

        public Task<string> CreateRewardAsync(TwitchRewardSettings settings, CancellationToken cancellationToken)
        {
            if (CreateError is not null)
            {
                throw CreateError;
            }

            Created.Add(settings);
            return Task.FromResult($"created-{Created.Count}");
        }

        public Task UpdateRewardAsync(string rewardId, TwitchRewardSettings settings, CancellationToken cancellationToken)
        {
            if (UpdateError is not null)
            {
                throw UpdateError;
            }

            Updated.Add((rewardId, settings));
            return Task.CompletedTask;
        }

        public Task SetRedemptionStatusAsync(string rewardId, string redemptionId, string status, CancellationToken cancellationToken)
        {
            lock (Statuses)
            {
                Statuses.Add($"{rewardId}/{redemptionId}={status}");
            }

            return Task.CompletedTask;
        }
    }
}

public class TwitchAuthTests
{
    private static readonly Uri Id = new("https://id.test/");
    private static readonly Uri Api = new("https://api.test/");

    private static TwitchDeviceCode Device => new("device", "CODE", "https://twitch.test/activate", 60, 0);

    private static string TempFile() =>
        Path.Combine(Path.GetTempPath(), "ds3-cc-tests", Guid.NewGuid().ToString("N"), "twitch.bin");

    [Fact]
    public async Task Sign_in_waits_for_approval_then_reads_the_channel()
    {
        var polls = 0;
        var http = new HttpClient(new StubHandler(request => request.RequestUri!.AbsolutePath switch
        {
            "/oauth2/token" => ++polls < 3
                ? (HttpStatusCode.BadRequest, """{"status":400,"message":"authorization_pending"}""")
                : (HttpStatusCode.OK, """{"access_token":"access","refresh_token":"refresh"}"""),
            "/oauth2/validate" => (HttpStatusCode.OK, """{"user_id":"42","login":"streamer"}"""),
            _ => (HttpStatusCode.NotFound, "{}")
        }));

        var tokens = await new TwitchAuth(http, Id, "client").WaitForApprovalAsync(Device, default);

        Assert.Equal(3, polls);
        Assert.Equal("access", tokens.AccessToken);
        Assert.Equal("42", tokens.UserId);
        Assert.Equal("streamer", tokens.Login);
    }

    [Fact]
    public async Task A_denied_sign_in_stops_waiting()
    {
        var http = new HttpClient(new StubHandler(_ =>
            (HttpStatusCode.BadRequest, """{"status":400,"message":"invalid device code"}""")));

        await Assert.ThrowsAsync<TwitchException>(() =>
            new TwitchAuth(http, Id, "client").WaitForApprovalAsync(Device, default));
    }

    [Fact]
    public void Tokens_survive_a_restart_and_are_not_plain_text()
    {
        var path = TempFile();
        var store = new TwitchTokenStore(path);

        store.Save(new TwitchTokens { AccessToken = "visible-access-token", RefreshToken = "r", UserId = "1", Login = "l" });

        Assert.Equal("visible-access-token", new TwitchTokenStore(path).Load()!.AccessToken);
        Assert.DoesNotContain("visible-access-token", Encoding.UTF8.GetString(File.ReadAllBytes(path)));
    }

    [Fact]
    public void A_damaged_token_file_counts_as_signed_out()
    {
        var path = TempFile();
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllBytes(path, [1, 2, 3]);

        Assert.Null(new TwitchTokenStore(path).Load());
    }

    [Fact]
    public async Task An_expired_token_is_refreshed_saved_and_retried()
    {
        var path = TempFile();
        var store = new TwitchTokenStore(path);
        var refreshes = 0;
        var http = new HttpClient(new StubHandler(request =>
        {
            if (request.RequestUri!.AbsolutePath == "/oauth2/token")
            {
                refreshes++;
                return (HttpStatusCode.OK, """{"access_token":"new","refresh_token":"new-refresh"}""");
            }

            return request.Headers.GetValues("Authorization").Single() == "Bearer new"
                ? (HttpStatusCode.OK, """{"data":[]}""")
                : (HttpStatusCode.Unauthorized, """{"message":"expired"}""");
        }));
        var tokens = new TwitchTokens { AccessToken = "old", RefreshToken = "old-refresh", UserId = "42", Login = "s" };
        var session = new TwitchSession(new TwitchAuth(http, Id, "client"), store, tokens);
        var api = new TwitchApi(http, Api, "client", session);

        await Task.WhenAll(api.GetManageableRewardsAsync(default), api.GetManageableRewardsAsync(default));

        Assert.Equal(1, refreshes);
        Assert.Equal("new-refresh", store.Load()!.RefreshToken);
    }

    [Fact]
    public async Task Reward_settings_are_read_from_the_list()
    {
        var http = new HttpClient(new StubHandler(_ => (HttpStatusCode.OK, """
            {"data":[
              {"id":"a","title":"Blind","cost":500,"is_enabled":true,"global_cooldown_setting":{"is_enabled":true,"global_cooldown_seconds":90}},
              {"id":"b","title":"Kill","cost":9000,"is_enabled":false,"global_cooldown_setting":{"is_enabled":false,"global_cooldown_seconds":60}}
            ]}
            """)));
        var tokens = new TwitchTokens { AccessToken = "a", RefreshToken = "r", UserId = "42", Login = "s" };
        var session = new TwitchSession(new TwitchAuth(http, Id, "client"), new TwitchTokenStore(TempFile()), tokens);

        var rewards = await new TwitchApi(http, Api, "client", session).GetManageableRewardsAsync(default);

        Assert.Equal(new TwitchReward("a", "Blind", 500, true, 90), rewards[0]);
        Assert.Equal(new TwitchReward("b", "Kill", 9000, false, 0), rewards[1]);
    }

    [Fact]
    public async Task Redemption_updates_name_the_reward_and_the_status()
    {
        HttpRequestMessage? sent = null;
        string? body = null;
        var http = new HttpClient(new StubHandler(request =>
        {
            sent = request;
            body = request.Content!.ReadAsStringAsync().Result;
            return (HttpStatusCode.OK, """{"data":[]}""");
        }));
        var tokens = new TwitchTokens { AccessToken = "a", RefreshToken = "r", UserId = "42", Login = "s" };
        var session = new TwitchSession(new TwitchAuth(http, Id, "client"), new TwitchTokenStore(TempFile()), tokens);

        await new TwitchApi(http, Api, "client", session).SetRedemptionStatusAsync("reward", "redeem", "CANCELED", default);

        Assert.Equal(HttpMethod.Patch, sent!.Method);
        Assert.Contains("reward_id=reward", sent.RequestUri!.Query);
        Assert.Contains("id=redeem", sent.RequestUri.Query);
        Assert.Contains("broadcaster_id=42", sent.RequestUri.Query);
        Assert.Equal("client", sent.Headers.GetValues("Client-Id").Single());
        Assert.Equal("""{"status":"CANCELED"}""", body);
    }

    private sealed class StubHandler(Func<HttpRequestMessage, (HttpStatusCode Status, string Body)> respond) : HttpMessageHandler
    {
        protected override Task<HttpResponseMessage> SendAsync(HttpRequestMessage request, CancellationToken cancellationToken)
        {
            var (status, body) = respond(request);
            return Task.FromResult(new HttpResponseMessage(status) { Content = new StringContent(body) });
        }
    }
}

public sealed class TwitchEventSubTests : IDisposable
{
    private readonly HttpListener _server = new();
    private readonly Uri _url;
    private readonly List<string> _subscribed = [];
    private readonly List<string> _events = [];
    private readonly CancellationTokenSource _stop = new(TimeSpan.FromSeconds(20));
    private readonly TwitchEventSubClient _client;

    public TwitchEventSubTests()
    {
        var probe = new TcpListener(IPAddress.Loopback, 0);
        probe.Start();
        var port = ((IPEndPoint)probe.LocalEndpoint).Port;
        probe.Stop();
        _server.Prefixes.Add($"http://localhost:{port}/");
        _server.Start();
        _url = new Uri($"ws://localhost:{port}/ws");
        _client = new TwitchEventSubClient(
            _url,
            (sessionId, _) =>
            {
                lock (_subscribed)
                {
                    _subscribed.Add(sessionId);
                }

                return Task.CompletedTask;
            },
            _ => { })
        {
            FirstRetryDelay = TimeSpan.FromMilliseconds(10)
        };
        _client.Notification += (type, payload) =>
        {
            lock (_events)
            {
                _events.Add($"{type}:{payload.GetProperty("id").GetString()}");
            }
        };
    }

    public void Dispose()
    {
        _stop.Cancel();
        _server.Close();
    }

    private async Task<WebSocket> AcceptAsync()
    {
        var context = await _server.GetContextAsync().WaitAsync(_stop.Token);
        var accepted = await context.AcceptWebSocketAsync(null);
        return accepted.WebSocket;
    }

    private Task SendAsync(WebSocket socket, string text) =>
        socket.SendAsync(Encoding.UTF8.GetBytes(text), WebSocketMessageType.Text, true, _stop.Token);

    private static string Welcome(string sessionId) => $$$$"""
        {"metadata":{"message_id":"w-{{{{sessionId}}}}","message_type":"session_welcome"},
         "payload":{"session":{"id":"{{{{sessionId}}}}","keepalive_timeout_seconds":10,"reconnect_url":null}}}
        """;

    private static string Notification(string messageId, string eventId) => $$$$"""
        {"metadata":{"message_id":"{{{{messageId}}}}","message_type":"notification"},
         "payload":{"subscription":{"type":"test.event"},"event":{"id":"{{{{eventId}}}}"}}}
        """;

    private static string Reconnect(Uri url) => $$$$"""
        {"metadata":{"message_id":"r","message_type":"session_reconnect"},
         "payload":{"session":{"id":"s","reconnect_url":"{{{{url}}}}"}}}
        """;

    private async Task WaitUntilAsync(Func<bool> ready)
    {
        while (!ready())
        {
            await Task.Delay(10, _stop.Token);
        }
    }

    private int SubscribedCount
    {
        get
        {
            lock (_subscribed)
            {
                return _subscribed.Count;
            }
        }
    }

    private int EventCount
    {
        get
        {
            lock (_events)
            {
                return _events.Count;
            }
        }
    }

    [Fact]
    public async Task It_subscribes_on_welcome_and_delivers_events_once()
    {
        _ = _client.RunAsync(_stop.Token);
        var socket = await AcceptAsync();

        await SendAsync(socket, Welcome("first"));
        await WaitUntilAsync(() => SubscribedCount == 1);
        await SendAsync(socket, Notification("m1", "a"));
        await SendAsync(socket, Notification("m1", "a"));
        await SendAsync(socket, Notification("m2", "b"));
        await WaitUntilAsync(() => EventCount == 2);

        Assert.Equal(["first"], _subscribed);
        Assert.Equal(["test.event:a", "test.event:b"], _events);
    }

    [Fact]
    public async Task A_lost_connection_reconnects_and_subscribes_again()
    {
        _ = _client.RunAsync(_stop.Token);
        var first = await AcceptAsync();
        await SendAsync(first, Welcome("first"));
        await WaitUntilAsync(() => SubscribedCount == 1);

        first.Abort();
        var second = await AcceptAsync();
        await SendAsync(second, Welcome("second"));
        await WaitUntilAsync(() => SubscribedCount == 2);

        Assert.Equal(["first", "second"], _subscribed);
    }

    [Fact]
    public async Task A_reconnect_message_moves_across_without_subscribing_again()
    {
        _ = _client.RunAsync(_stop.Token);
        var first = await AcceptAsync();
        await SendAsync(first, Welcome("first"));
        await WaitUntilAsync(() => SubscribedCount == 1);

        await SendAsync(first, Reconnect(_url));
        var second = await AcceptAsync();
        await SendAsync(second, Welcome("second"));
        await SendAsync(first, Notification("late-on-old", "old"));
        await SendAsync(second, Notification("on-new", "new"));
        await WaitUntilAsync(() => EventCount == 2);
        first.Abort();
        await SendAsync(second, Notification("after-close", "after"));
        await WaitUntilAsync(() => EventCount == 3);

        Assert.Equal(["first"], _subscribed);
    }

    [Fact]
    public async Task A_refused_subscription_stops_retrying()
    {
        var client = new TwitchEventSubClient(
            _url,
            (_, _) => throw new TwitchException("not affiliate", HttpStatusCode.Forbidden),
            _ => { });
        var running = client.RunAsync(_stop.Token);
        var socket = await AcceptAsync();

        await SendAsync(socket, Welcome("first"));

        await Assert.ThrowsAsync<TwitchException>(() => running);
    }
}
