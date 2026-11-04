using System.Text.Json;

namespace Ds3CrowdControl.Core;

public sealed class EffectQueue
{
    private readonly Catalog _catalog;
    private readonly GameBridgeServer _bridge;
    private readonly Dictionary<string, DateTimeOffset> _lastSuccess = [];
    private readonly object _gate = new();

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

        if (!preset.Enabled)
        {
            return new EffectResult("", EffectStatus.Failed, $"{preset.Name} is disabled");
        }

        lock (_gate)
        {
            if (_lastSuccess.TryGetValue(preset.Id, out var last) && preset.CooldownSeconds > 0)
            {
                var readyAt = last.AddSeconds(preset.CooldownSeconds);
                if (DateTimeOffset.UtcNow < readyAt)
                {
                    var remaining = (int)Math.Ceiling((readyAt - DateTimeOffset.UtcNow).TotalSeconds);
                    return new EffectResult("", EffectStatus.Failed, $"{preset.Name} is on cooldown for {remaining}s");
                }
            }
        }

        if (!_bridge.State.IsSafe)
        {
            return new EffectResult("", EffectStatus.Unavailable, UnsafeReason(_bridge.State));
        }

        var args = ResolveArguments(preset);
        var result = await _bridge.TriggerAsync(preset.HandlerId, args, cancellationToken).ConfigureAwait(false);
        if (result.Status == EffectStatus.Success)
        {
            lock (_gate)
            {
                _lastSuccess[preset.Id] = DateTimeOffset.UtcNow;
            }
        }

        _bridge.Log($"{source}: {preset.Name} -> {result.Status}: {result.Message}");
        return result;
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
}
