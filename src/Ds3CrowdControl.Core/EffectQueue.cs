using System.Text.Json;

namespace Ds3CrowdControl.Core;

/// <summary>One effect that has not finished yet, as the queue panel shows it.</summary>
public sealed record QueuedEffect(string Id, string Name, string Who, string State);

public sealed class EffectQueue
{
    private const string ManualSource = "manual";
    private const string TwitchSourcePrefix = "twitch:";
    private const string PausedReason = "paused";
    private const string GapReason = "gap between effects";

    private readonly Catalog _catalog;
    private readonly GameBridgeServer _bridge;
    private readonly List<Entry> _entries = [];
    private readonly object _gate = new();
    private readonly SemaphoreSlim _turn = new(1, 1);
    private DateTimeOffset _lastStart = DateTimeOffset.MinValue;
    private volatile bool _paused;

    /// <summary>How long an effect waits for a safe moment before giving up.</summary>
    public TimeSpan ReadyTimeout { get; set; } = TimeSpan.FromMinutes(5);

    public TimeSpan ReadyPollInterval { get; set; } = TimeSpan.FromMilliseconds(500);

    /// <summary>The shortest time from one effect starting to the next. Test skips it.</summary>
    public TimeSpan MinGap { get; set; }

    /// <summary>Holds every waiting effect in place until it is turned off again.</summary>
    public bool Paused
    {
        get => _paused;
        set => _paused = value;
    }

    /// <summary>Runs when an effect joins, leaves or changes state. Can run on any thread.</summary>
    public event Action? Changed;

    /// <summary>Waiting details that name an effect before it happens, kept out of the on-screen log.</summary>
    public Action<string>? Trace { get; set; }

    public EffectQueue(Catalog catalog, GameBridgeServer bridge)
    {
        _catalog = catalog;
        _bridge = bridge;
    }

    public async Task<EffectResult> EnqueueAsync(string presetId, string source, CancellationToken cancellationToken = default)
    {
        EffectPreset? preset;
        lock (_gate)
        {
            preset = _catalog.FindPreset(presetId);
        }

        if (preset is null)
        {
            return new EffectResult("", EffectStatus.Unavailable, "that preset does not exist");
        }

        if (!_bridge.State.CanWait)
        {
            return new EffectResult("", EffectStatus.Unavailable, UnsafeReason(_bridge.State));
        }

        var args = ResolveArguments(preset);
        using var entry = new Entry(preset.Name, source, cancellationToken);
        Add(entry);
        EffectResult result;
        try
        {
            result = await RunWhenReadyAsync(entry, preset, source, args, cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (entry.Cleared)
        {
            result = new EffectResult("", EffectStatus.Failed, "cleared from the queue");
        }
        finally
        {
            Remove(entry);
        }

        _bridge.Log($"{source}: {preset.Name} -> {result.Status}: {result.Message}");
        return result;
    }

    public IReadOnlyList<QueuedEffect> Snapshot()
    {
        lock (_gate)
        {
            return _entries.Select(entry => new QueuedEffect(entry.Id, entry.Name, Who(entry.Source), entry.State)).ToList();
        }
    }

    /// <summary>
    /// Removes everything not yet sent to the game, and returns how many. Those effects
    /// finish as failed, so a redeem gets its points back.
    /// </summary>
    public int Clear()
    {
        List<Entry> cleared;
        lock (_gate)
        {
            cleared = _entries.Where(entry => !entry.Sent).ToList();
            foreach (var entry in cleared)
            {
                entry.Cleared = true;
            }
        }

        foreach (var entry in cleared)
        {
            entry.Cancel();
        }

        return cleared.Count;
    }

    public static string Who(string source)
    {
        if (source == ManualSource)
        {
            return "Test";
        }

        return source.StartsWith(TwitchSourcePrefix, StringComparison.Ordinal)
            ? source[TwitchSourcePrefix.Length..]
            : source;
    }

    /// <summary>
    /// Runs effects one at a time. Further effects line up behind the one that is
    /// waiting, and each is asked for again once it is that effect's turn, so nothing
    /// fires during a load or a death.
    /// </summary>
    private async Task<EffectResult> RunWhenReadyAsync(Entry entry, EffectPreset preset, string source, JsonElement args, CancellationToken cancellationToken)
    {
        if (!await _turn.WaitAsync(0, entry.Token).ConfigureAwait(false))
        {
            Trace?.Invoke($"{source}: {preset.Name} is queued and will run after the one already waiting");
            await _turn.WaitAsync(entry.Token).ConfigureAwait(false);
        }

        try
        {
            SetState(entry, "next");
            var deadline = DateTimeOffset.UtcNow + ReadyTimeout;
            string? announced = null;
            while (true)
            {
                entry.Token.ThrowIfCancellationRequested();
                string reason;
                if (_paused)
                {
                    reason = PausedReason;
                    deadline = DateTimeOffset.UtcNow + ReadyTimeout;
                }
                else
                {
                    var state = _bridge.State;
                    if (!state.CanWait)
                    {
                        return new EffectResult("", EffectStatus.Unavailable, UnsafeReason(state));
                    }

                    if (source != ManualSource && InGap())
                    {
                        reason = GapReason;
                        deadline = DateTimeOffset.UtcNow + ReadyTimeout;
                    }
                    else if (state.IsSafe)
                    {
                        var result = await SendAsync(entry, preset, args, cancellationToken).ConfigureAwait(false);
                        if (result.Status != EffectStatus.Retry)
                        {
                            return result;
                        }

                        reason = result.Message;
                    }
                    else
                    {
                        reason = "no character is loaded";
                    }
                }

                if (DateTimeOffset.UtcNow >= deadline)
                {
                    return new EffectResult("", EffectStatus.Failed, $"gave up waiting: {reason}");
                }

                if (announced != reason)
                {
                    announced = reason;
                    SetState(entry, reason);
                    Trace?.Invoke($"{source}: {preset.Name} is waiting: {reason}");
                }

                await Task.Delay(ReadyPollInterval, entry.Token).ConfigureAwait(false);
            }
        }
        finally
        {
            _turn.Release();
        }
    }

    /// <summary>Uses the caller's token, because Clear must not abandon an effect the game already has.</summary>
    private async Task<EffectResult> SendAsync(Entry entry, EffectPreset preset, JsonElement args, CancellationToken cancellationToken)
    {
        lock (_gate)
        {
            if (entry.Cleared)
            {
                throw new OperationCanceledException();
            }

            entry.Sent = true;
            entry.State = "running";
        }

        Changed?.Invoke();
        var startedAt = DateTimeOffset.UtcNow;
        var result = await _bridge.TriggerAsync(preset.HandlerId, args, cancellationToken).ConfigureAwait(false);
        lock (_gate)
        {
            entry.Sent = false;
            if (result.Status == EffectStatus.Success)
            {
                _lastStart = startedAt;
            }
        }

        return result;
    }

    private bool InGap()
    {
        lock (_gate)
        {
            return MinGap > TimeSpan.Zero && DateTimeOffset.UtcNow < _lastStart + MinGap;
        }
    }

    private void Add(Entry entry)
    {
        lock (_gate)
        {
            _entries.Add(entry);
        }

        Changed?.Invoke();
    }

    private void Remove(Entry entry)
    {
        lock (_gate)
        {
            _entries.Remove(entry);
        }

        Changed?.Invoke();
    }

    private void SetState(Entry entry, string state)
    {
        lock (_gate)
        {
            entry.State = state;
        }

        Changed?.Invoke();
    }

    public static string UnsafeReason(GameState state)
    {
        if (!state.Connected)
        {
            return "the game mod is not connected";
        }

        if (!state.ExeMatch)
        {
            return "this Dark Souls III build is not supported";
        }

        if (!state.SessionKnown)
        {
            return "the session type is unknown, so effects stay off";
        }

        if (!state.Offline)
        {
            return "effects run only while the game is in an offline session";
        }

        return "there is no local player yet";
    }

    private JsonElement ResolveArguments(EffectPreset preset)
    {
        var handler = _catalog.FindHandler(preset.HandlerId);
        var merged = handler is null
            ? new Dictionary<string, JsonElement>(preset.Arguments)
            : BuiltinHandlers.DefaultArguments(handler);
        foreach (var pair in preset.Arguments)
        {
            merged[pair.Key] = pair.Value;
        }

        return JsonSerializer.SerializeToElement(merged);
    }

    private sealed class Entry(string name, string source, CancellationToken callerToken) : IDisposable
    {
        private readonly CancellationTokenSource _cancel = CancellationTokenSource.CreateLinkedTokenSource(callerToken);

        public string Id { get; } = Guid.NewGuid().ToString("N");
        public string Name { get; } = name;
        public string Source { get; } = source;
        public string State { get; set; } = "queued";
        public bool Sent { get; set; }
        public bool Cleared { get; set; }
        public CancellationToken Token => _cancel.Token;

        public void Cancel() => _cancel.Cancel();

        public void Dispose() => _cancel.Dispose();
    }
}
