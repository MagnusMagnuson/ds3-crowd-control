using System.Net;
using System.Text.Json;

namespace Ds3CrowdControl.Core;

/// <summary>
/// Turns redeems, Bits and gift subs into effects. Failed redeems are refunded. Twitch has
/// no refund for Bits or gift subs, so those failures are only logged.
/// </summary>
public sealed class TwitchTriggerSource(Catalog catalog, Action<string> log) : ITriggerSource
{
    public const string RedemptionEvent = "channel.channel_points_custom_reward_redemption.add";
    public const string BitsEvent = "channel.bits.use";
    /// <summary>Only the Twitch CLI sends this here. Real Bits arrive as <see cref="BitsEvent"/>.</summary>
    public const string CheerEvent = "channel.cheer";
    public const string GiftEvent = "channel.subscription.gift";
    private const string Fulfilled = "FULFILLED";
    private const string Canceled = "CANCELED";
    private const int MaxRewardTitleLength = 45;
    private const string AnonymousViewer = "Anonymous";

    public static IReadOnlyList<string> EventTypes { get; } = [RedemptionEvent, BitsEvent, GiftEvent];

    private int _waiting;

    /// <summary>More than this waiting at once get refunded. Zero means no limit.</summary>
    public int WaitingLimit { get; set; }

    /// <summary>
    /// For the Twitch CLI's test server, which knows nothing about the real rewards: a
    /// redeem finds its event by title, and any status counts.
    /// </summary>
    public bool TestMode { get; set; }

    /// <summary>Hides the rewards on the next sync, so viewers cannot spend points during a pause.</summary>
    public bool Paused { get; set; }

    public string Name => "twitch";

    public ITwitchRewards? Rewards { get; set; }

    public Func<string, string, CancellationToken, Task<EffectResult>>? RunEffect { get; set; }

    public void Start(EffectQueue queue) => RunEffect = queue.EnqueueAsync;

    public void Stop() => RunEffect = null;

    public static string RewardTitle(EffectPreset preset) =>
        preset.Name.Length <= MaxRewardTitleLength ? preset.Name : preset.Name[..MaxRewardTitleLength];

    public async Task HandleRedemptionAsync(JsonElement redemption, CancellationToken cancellationToken)
    {
        var rewards = Rewards;
        var reward = redemption.GetProperty("reward");
        var rewardId = reward.GetProperty("id").GetString()!;
        var title = TestMode ? reward.GetProperty("title").GetString() : null;
        var preset = catalog.Presets.FirstOrDefault(item =>
            item.TwitchRewardId == rewardId
            || string.Equals(RewardTitle(item), title, StringComparison.OrdinalIgnoreCase));
        if (preset is null || rewards is null)
        {
            return;
        }

        if (!TestMode && !string.Equals(redemption.GetProperty("status").GetString(), "unfulfilled", StringComparison.OrdinalIgnoreCase))
        {
            return;
        }

        var redemptionId = redemption.GetProperty("id").GetString()!;
        var viewer = redemption.GetProperty("user_name").GetString();
        var status = preset.RedeemEnabled
            ? await RunAsync(preset, viewer, cancellationToken).ConfigureAwait(false)
            : Canceled;
        try
        {
            await rewards.SetRedemptionStatusAsync(rewardId, redemptionId, status, cancellationToken).ConfigureAwait(false);
        }
        catch (Exception exception) when (exception is TwitchException or HttpRequestException)
        {
            log($"Twitch: could not mark {viewer}'s redemption {status}: {exception.Message}");
        }
    }

    public Task HandleBitsAsync(JsonElement cheer, CancellationToken cancellationToken)
    {
        var bits = cheer.GetProperty("bits").GetInt32();
        return RunPaidAsync(
            preset => preset.BitsEnabled && preset.BitsAmount == bits,
            Viewer(cheer),
            $"{bits} Bits",
            cancellationToken);
    }

    public Task HandleGiftAsync(JsonElement gift, CancellationToken cancellationToken)
    {
        var total = gift.GetProperty("total").GetInt32();
        return RunPaidAsync(
            preset => preset.GiftEnabled && preset.GiftAmount == total,
            Viewer(gift),
            total == 1 ? "1 gift sub" : $"{total} gift subs",
            cancellationToken);
    }

    private static string Viewer(JsonElement paid) =>
        paid.TryGetProperty("user_name", out var name) && name.ValueKind == JsonValueKind.String
            ? name.GetString()!
            : AnonymousViewer;

    /// <summary>Only the exact amount runs an event. Events sharing an amount are picked at random.</summary>
    private async Task RunPaidAsync(Func<EffectPreset, bool> matches, string viewer, string paid, CancellationToken cancellationToken)
    {
        var choices = catalog.Presets.Where(matches).ToList();
        var run = RunEffect;
        if (choices.Count == 0 || run is null)
        {
            return;
        }

        var preset = choices[Random.Shared.Next(choices.Count)];
        try
        {
            var result = await run(preset.Id, $"twitch:{viewer}", cancellationToken).ConfigureAwait(false);
            if (result.Status != EffectStatus.Success)
            {
                log($"Twitch: {viewer}'s {paid} did not run {preset.Name}, and Twitch cannot refund it: {result.Message}");
            }
        }
        catch (Exception exception) when (exception is not OperationCanceledException)
        {
            log($"Twitch: {viewer}'s {paid} did not run {preset.Name}, and Twitch cannot refund it: {exception.Message}");
        }
    }

    private async Task<string> RunAsync(EffectPreset preset, string? viewer, CancellationToken cancellationToken)
    {
        var run = RunEffect;
        if (run is null)
        {
            return Canceled;
        }

        var limit = WaitingLimit;
        if (Interlocked.Increment(ref _waiting) > limit && limit > 0)
        {
            Interlocked.Decrement(ref _waiting);
            log($"Twitch: refunded {viewer}'s {preset.Name}, too many effects are waiting");
            return Canceled;
        }

        try
        {
            var result = await run(preset.Id, $"twitch:{viewer}", cancellationToken).ConfigureAwait(false);
            if (result.Status == EffectStatus.Success)
            {
                return Fulfilled;
            }

            log($"Twitch: refunded {viewer}'s {preset.Name}: {result.Message}");
            return Canceled;
        }
        catch (Exception exception) when (exception is not OperationCanceledException)
        {
            log($"Twitch: refunded {viewer}'s {preset.Name}: {exception.Message}");
            return Canceled;
        }
        finally
        {
            Interlocked.Decrement(ref _waiting);
        }
    }

    /// <summary>
    /// Makes Twitch match the presets. Rewards are only ever hidden, never deleted, so the
    /// streamer's icons and colours survive. Only rewards this app made are visible here, so
    /// the streamer's own rewards are never touched. Returns true when a reward id changed.
    /// </summary>
    public async Task<bool> SyncRewardsAsync(CancellationToken cancellationToken)
    {
        var rewards = Rewards ?? throw new TwitchException("Twitch is not connected");
        var existing = await rewards.GetManageableRewardsAsync(cancellationToken).ConfigureAwait(false);
        var kept = new HashSet<string>();
        var changed = false;
        foreach (var preset in catalog.Presets)
        {
            var title = RewardTitle(preset);
            var settings = new TwitchRewardSettings(title, Math.Max(preset.RedeemCost, 1), preset.RedeemEnabled && !Paused, preset.CooldownSeconds);
            var match = existing.FirstOrDefault(reward => reward.Id == preset.TwitchRewardId)
                ?? existing.FirstOrDefault(reward => string.Equals(reward.Title, title, StringComparison.OrdinalIgnoreCase));
            if (match is null && !preset.RedeemEnabled)
            {
                changed |= preset.TwitchRewardId is not null;
                preset.TwitchRewardId = null;
                continue;
            }

            if (match is not null)
            {
                kept.Add(match.Id);
            }

            try
            {
                var rewardId = match?.Id;
                if (rewardId is null)
                {
                    rewardId = await rewards.CreateRewardAsync(settings, cancellationToken).ConfigureAwait(false);
                }
                else if (!match!.Matches(settings))
                {
                    await rewards.UpdateRewardAsync(rewardId, settings, cancellationToken).ConfigureAwait(false);
                }

                kept.Add(rewardId);
                changed |= preset.TwitchRewardId != rewardId;
                preset.TwitchRewardId = rewardId;
            }
            catch (TwitchException exception)
            {
                log($"Twitch: reward for {preset.Name} failed: {Explain(exception)}");
                if (exception.Status == HttpStatusCode.Forbidden)
                {
                    return changed;
                }
            }
        }

        foreach (var reward in existing.Where(reward => reward.Enabled && !kept.Contains(reward.Id)))
        {
            try
            {
                var hidden = new TwitchRewardSettings(reward.Title, reward.Cost, false, reward.CooldownSeconds);
                await rewards.UpdateRewardAsync(reward.Id, hidden, cancellationToken).ConfigureAwait(false);
            }
            catch (TwitchException exception)
            {
                log($"Twitch: could not hide reward {reward.Title}: {exception.Message}");
            }
        }

        var live = catalog.Presets.Count(preset => preset.RedeemEnabled && preset.TwitchRewardId is not null);
        log($"Twitch: {(Paused ? 0 : live)} redeems are live");
        return changed;
    }

    public async Task PauseRewardsAsync(CancellationToken cancellationToken)
    {
        var rewards = Rewards;
        if (rewards is null)
        {
            return;
        }

        foreach (var preset in catalog.Presets.Where(item => item.RedeemEnabled && item.TwitchRewardId is not null))
        {
            var settings = new TwitchRewardSettings(RewardTitle(preset), Math.Max(preset.RedeemCost, 1), false, preset.CooldownSeconds);
            await rewards.UpdateRewardAsync(preset.TwitchRewardId!, settings, cancellationToken).ConfigureAwait(false);
        }
    }

    public static string Explain(TwitchException exception) =>
        exception.Status == HttpStatusCode.Forbidden
            ? $"{exception.Message} (channel points need an Affiliate or Partner channel)"
            : exception.Message;
}
