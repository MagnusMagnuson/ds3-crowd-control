using System.Text.Json;

namespace Ds3CrowdControl.Core;

/// <summary>Owns the sign-in and the live connection, and hands redemptions to the trigger source.</summary>
public sealed class TwitchConnection(
    TwitchTriggerSource source,
    TwitchTokenStore store,
    HttpClient http,
    Action<string> log)
{
    private static readonly TimeSpan ValidateEvery = TimeSpan.FromHours(1);
    private static readonly TimeSpan SyncDelay = TimeSpan.FromSeconds(2);

    private readonly TwitchAuth _auth = new(http, TwitchConfig.IdUrl, TwitchConfig.ClientId);
    private readonly SemaphoreSlim _syncing = new(1, 1);
    private Debouncer? _syncSoon;
    private CancellationTokenSource? _running;
    private TwitchSession? _session;

    /// <summary>The channel that is signed in, or empty.</summary>
    public string Channel => _session?.Tokens.Login ?? "";

    public bool SignedIn => _session is not null;

    public event Action<string>? StatusChanged;

    /// <summary>Runs after a sync changed reward ids, so they can be saved.</summary>
    public event Action? RewardsChanged;

    /// <summary>Picks up a saved sign-in when the manager starts.</summary>
    public Task ResumeAsync()
    {
        var tokens = store.Load();
        if (tokens is not null)
        {
            Begin(tokens);
        }

        return Task.CompletedTask;
    }

    public async Task SignInAsync(Action<TwitchDeviceCode> showCode, CancellationToken cancellationToken)
    {
        var device = await _auth.StartDeviceAsync(cancellationToken).ConfigureAwait(false);
        showCode(device);
        var tokens = await _auth.WaitForApprovalAsync(device, cancellationToken).ConfigureAwait(false);
        store.Save(tokens);
        Begin(tokens);
    }

    public async Task SignOutAsync()
    {
        await PauseRewardsAsync().ConfigureAwait(false);
        Stop();
        store.Delete();
        StatusChanged?.Invoke("Twitch: not connected");
    }

    /// <summary>Waits for clicks to stop, so a burst of changes reaches Twitch as one update.</summary>
    public void RequestSync() =>
        (_syncSoon ??= new Debouncer(SyncDelay, () => SyncAsync(CancellationToken.None))).Request();

    /// <summary>One at a time, because two syncs together could both create the same reward.</summary>
    public async Task SyncAsync(CancellationToken cancellationToken)
    {
        if (!SignedIn)
        {
            return;
        }

        await _syncing.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            if (await source.SyncRewardsAsync(cancellationToken).ConfigureAwait(false))
            {
                RewardsChanged?.Invoke();
            }
        }
        catch (TwitchException exception)
        {
            log($"Twitch: rewards were not updated: {TwitchTriggerSource.Explain(exception)}");
        }
        catch (HttpRequestException exception)
        {
            log($"Twitch: rewards were not updated: {exception.Message}");
        }
        finally
        {
            _syncing.Release();
        }
    }

    /// <summary>Stops viewers spending points on rewards nothing is listening to.</summary>
    public async Task PauseRewardsAsync()
    {
        _syncSoon?.Cancel();
        if (!SignedIn)
        {
            return;
        }

        await _syncing.WaitAsync().ConfigureAwait(false);
        try
        {
            await source.PauseRewardsAsync(CancellationToken.None).ConfigureAwait(false);
        }
        catch (Exception exception) when (exception is TwitchException or HttpRequestException)
        {
            log($"Twitch: rewards were not paused: {exception.Message}");
        }
        finally
        {
            _syncing.Release();
        }
    }

    /// <summary>
    /// Listens to the Twitch CLI's local server instead of Twitch. It sends events to
    /// anyone connected, so this needs no sign-in or subscriptions.
    /// </summary>
    public void StartTestServer()
    {
        Stop();
        source.TestMode = true;
        source.Rewards = new TestRewards(log);
        var running = new CancellationTokenSource();
        _running = running;
        var feed = new TwitchEventSubClient(TwitchConfig.TestServerUrl, (_, _) => Task.CompletedTask, log);
        feed.Notification += (type, payload) => OnNotification(type, payload, running.Token);
        feed.Connected += () => StatusChanged?.Invoke("Twitch: connected to the test server");
        feed.Disconnected += () => StatusChanged?.Invoke("Twitch: waiting for the test server");
        StatusChanged?.Invoke("Twitch: waiting for the test server");
        _ = RunFeedAsync(feed, running.Token);
    }

    /// <summary>Goes back to the saved sign-in, if there is one.</summary>
    public Task StopTestServerAsync()
    {
        Stop();
        StatusChanged?.Invoke("Twitch: not connected");
        return ResumeAsync();
    }

    private void Begin(TwitchTokens tokens)
    {
        Stop();
        var session = new TwitchSession(_auth, store, tokens);
        var api = new TwitchApi(http, TwitchConfig.ApiUrl, TwitchConfig.ClientId, session);
        _session = session;
        source.Rewards = api;
        var running = new CancellationTokenSource();
        _running = running;
        var feed = new TwitchEventSubClient(
            TwitchConfig.EventSubUrl,
            (sessionId, token) => SubscribeAsync(api, sessionId, token),
            log);
        feed.Notification += (type, payload) => OnNotification(type, payload, running.Token);
        feed.Connected += () =>
        {
            StatusChanged?.Invoke($"Twitch: connected as {tokens.Login}");
            _ = SyncAsync(running.Token);
        };
        feed.Disconnected += () => StatusChanged?.Invoke("Twitch: reconnecting");
        StatusChanged?.Invoke("Twitch: connecting");
        _ = RunFeedAsync(feed, running.Token);
        _ = KeepValidatingAsync(session, running.Token);
    }

    private static async Task SubscribeAsync(TwitchApi api, string sessionId, CancellationToken cancellationToken)
    {
        foreach (var type in TwitchTriggerSource.EventTypes)
        {
            await api.SubscribeAsync(type, sessionId, cancellationToken).ConfigureAwait(false);
        }
    }

    private void OnNotification(string type, JsonElement payload, CancellationToken cancellationToken)
    {
        Func<JsonElement, CancellationToken, Task>? handle = type switch
        {
            TwitchTriggerSource.RedemptionEvent => source.HandleRedemptionAsync,
            TwitchTriggerSource.BitsEvent or TwitchTriggerSource.CheerEvent => source.HandleBitsAsync,
            TwitchTriggerSource.GiftEvent => source.HandleGiftAsync,
            _ => null
        };
        if (handle is not null)
        {
            _ = HandleAsync(handle, payload, cancellationToken);
        }
    }

    private async Task HandleAsync(Func<JsonElement, CancellationToken, Task> handle, JsonElement payload, CancellationToken cancellationToken)
    {
        try
        {
            await handle(payload, cancellationToken).ConfigureAwait(false);
        }
        catch (Exception exception) when (exception is not OperationCanceledException)
        {
            log($"Twitch: an event failed: {exception.Message}");
        }
    }

    private async Task RunFeedAsync(TwitchEventSubClient feed, CancellationToken cancellationToken)
    {
        try
        {
            await feed.RunAsync(cancellationToken).ConfigureAwait(false);
        }
        catch (TwitchException exception)
        {
            var message = TwitchTriggerSource.Explain(exception);
            log($"Twitch: could not listen for events: {message}");
            StatusChanged?.Invoke($"Twitch: {message}");
        }
        catch (OperationCanceledException)
        {
        }
    }

    private async Task KeepValidatingAsync(TwitchSession session, CancellationToken cancellationToken)
    {
        try
        {
            while (true)
            {
                var accessToken = session.Tokens.AccessToken;
                if (!await _auth.ValidateAsync(accessToken, cancellationToken).ConfigureAwait(false))
                {
                    await session.RefreshAsync(accessToken, cancellationToken).ConfigureAwait(false);
                }

                await Task.Delay(ValidateEvery, cancellationToken).ConfigureAwait(false);
            }
        }
        catch (OperationCanceledException)
        {
        }
        catch (Exception exception) when (exception is TwitchException or HttpRequestException)
        {
            log($"Twitch: sign-in check failed: {exception.Message}");
        }
    }

    private void Stop()
    {
        _running?.Cancel();
        _running?.Dispose();
        _running = null;
        _session = null;
        source.Rewards = null;
        source.TestMode = false;
    }

    /// <summary>Nothing here can reach Twitch, so a redeem only says what would have happened.</summary>
    private sealed class TestRewards(Action<string> log) : ITwitchRewards
    {
        public Task<IReadOnlyList<TwitchReward>> GetManageableRewardsAsync(CancellationToken cancellationToken) =>
            Task.FromResult<IReadOnlyList<TwitchReward>>([]);

        public Task<string> CreateRewardAsync(TwitchRewardSettings settings, CancellationToken cancellationToken) =>
            Task.FromResult("");

        public Task UpdateRewardAsync(string rewardId, TwitchRewardSettings settings, CancellationToken cancellationToken) =>
            Task.CompletedTask;

        public Task SetRedemptionStatusAsync(string rewardId, string redemptionId, string status, CancellationToken cancellationToken)
        {
            log($"Twitch test: the redeem would be {status}");
            return Task.CompletedTask;
        }
    }
}
