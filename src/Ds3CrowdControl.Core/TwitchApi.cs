using System.Net;
using System.Text;
using System.Text.Json;

namespace Ds3CrowdControl.Core;

public sealed record TwitchReward(string Id, string Title, int Cost = 0, bool Enabled = false, int CooldownSeconds = 0)
{
    public bool Matches(TwitchRewardSettings settings) =>
        Title == settings.Title
        && Cost == settings.Cost
        && Enabled == settings.Enabled
        && CooldownSeconds == settings.CooldownSeconds;
}

public sealed record TwitchRewardSettings(string Title, int Cost, bool Enabled, int CooldownSeconds);

/// <summary>What the trigger source needs from Twitch, so tests can stand in for it.</summary>
public interface ITwitchRewards
{
    Task<IReadOnlyList<TwitchReward>> GetManageableRewardsAsync(CancellationToken cancellationToken);

    Task<string> CreateRewardAsync(TwitchRewardSettings settings, CancellationToken cancellationToken);

    Task UpdateRewardAsync(string rewardId, TwitchRewardSettings settings, CancellationToken cancellationToken);

    Task SetRedemptionStatusAsync(string rewardId, string redemptionId, string status, CancellationToken cancellationToken);
}

/// <summary>Holds the current tokens and swaps them when they expire.</summary>
public sealed class TwitchSession(TwitchAuth auth, TwitchTokenStore store, TwitchTokens tokens)
{
    private readonly SemaphoreSlim _refreshing = new(1, 1);

    public TwitchTokens Tokens { get; private set; } = tokens;

    /// <summary>Two callers can hit an expired token together, and a refresh token works once.</summary>
    public async Task RefreshAsync(string expiredAccessToken, CancellationToken cancellationToken)
    {
        await _refreshing.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            if (Tokens.AccessToken != expiredAccessToken)
            {
                return;
            }

            Tokens = await auth.RefreshAsync(Tokens, cancellationToken).ConfigureAwait(false);
            store.Save(Tokens);
        }
        finally
        {
            _refreshing.Release();
        }
    }
}

public sealed class TwitchApi(HttpClient http, Uri apiUrl, string clientId, TwitchSession session) : ITwitchRewards
{
    private string Broadcaster => Uri.EscapeDataString(session.Tokens.UserId);

    public async Task<IReadOnlyList<TwitchReward>> GetManageableRewardsAsync(CancellationToken cancellationToken)
    {
        var body = await SendAsync(
            HttpMethod.Get,
            $"helix/channel_points/custom_rewards?broadcaster_id={Broadcaster}&only_manageable_rewards=true",
            null,
            cancellationToken).ConfigureAwait(false);
        using var document = JsonDocument.Parse(body);
        return document.RootElement.GetProperty("data").EnumerateArray()
            .Select(ReadReward)
            .ToList();
    }

    private static TwitchReward ReadReward(JsonElement reward)
    {
        var cooldown = reward.GetProperty("global_cooldown_setting");
        return new TwitchReward(
            reward.GetProperty("id").GetString()!,
            reward.GetProperty("title").GetString()!,
            reward.GetProperty("cost").GetInt32(),
            reward.GetProperty("is_enabled").GetBoolean(),
            cooldown.GetProperty("is_enabled").GetBoolean() ? cooldown.GetProperty("global_cooldown_seconds").GetInt32() : 0);
    }

    public async Task<string> CreateRewardAsync(TwitchRewardSettings settings, CancellationToken cancellationToken)
    {
        var body = await SendAsync(
            HttpMethod.Post,
            $"helix/channel_points/custom_rewards?broadcaster_id={Broadcaster}",
            RewardBody(settings),
            cancellationToken).ConfigureAwait(false);
        using var document = JsonDocument.Parse(body);
        return document.RootElement.GetProperty("data")[0].GetProperty("id").GetString()!;
    }

    public Task UpdateRewardAsync(string rewardId, TwitchRewardSettings settings, CancellationToken cancellationToken) =>
        SendAsync(
            HttpMethod.Patch,
            $"helix/channel_points/custom_rewards?broadcaster_id={Broadcaster}&id={Uri.EscapeDataString(rewardId)}",
            RewardBody(settings),
            cancellationToken);

    public Task SetRedemptionStatusAsync(string rewardId, string redemptionId, string status, CancellationToken cancellationToken) =>
        SendAsync(
            HttpMethod.Patch,
            "helix/channel_points/custom_rewards/redemptions"
                + $"?broadcaster_id={Broadcaster}&reward_id={Uri.EscapeDataString(rewardId)}&id={Uri.EscapeDataString(redemptionId)}",
            new { status },
            cancellationToken);

    public Task SubscribeAsync(string type, string sessionId, CancellationToken cancellationToken) =>
        SendAsync(
            HttpMethod.Post,
            "helix/eventsub/subscriptions",
            new
            {
                type,
                version = "1",
                condition = new { broadcaster_user_id = session.Tokens.UserId },
                transport = new { method = "websocket", session_id = sessionId }
            },
            cancellationToken);

    private static object RewardBody(TwitchRewardSettings settings) => new
    {
        title = settings.Title,
        cost = settings.Cost,
        is_enabled = settings.Enabled,
        is_global_cooldown_enabled = settings.CooldownSeconds > 0,
        global_cooldown_seconds = settings.CooldownSeconds,
        // Left in the queue so a failed effect can still refund the points.
        should_redemptions_skip_request_queue = false
    };

    private async Task<string> SendAsync(HttpMethod method, string path, object? body, CancellationToken cancellationToken)
    {
        var accessToken = session.Tokens.AccessToken;
        var (status, text) = await SendOnceAsync(method, path, body, accessToken, cancellationToken).ConfigureAwait(false);
        if (status == HttpStatusCode.Unauthorized)
        {
            await session.RefreshAsync(accessToken, cancellationToken).ConfigureAwait(false);
            (status, text) = await SendOnceAsync(method, path, body, session.Tokens.AccessToken, cancellationToken)
                .ConfigureAwait(false);
        }

        if ((int)status is < 200 or >= 300)
        {
            throw new TwitchException(TwitchAuth.ErrorMessage(text), status);
        }

        return text;
    }

    private async Task<(HttpStatusCode Status, string Body)> SendOnceAsync(
        HttpMethod method,
        string path,
        object? body,
        string accessToken,
        CancellationToken cancellationToken)
    {
        using var request = new HttpRequestMessage(method, new Uri(apiUrl, path));
        request.Headers.TryAddWithoutValidation("Authorization", $"Bearer {accessToken}");
        request.Headers.TryAddWithoutValidation("Client-Id", clientId);
        if (body is not null)
        {
            request.Content = new StringContent(JsonSerializer.Serialize(body), Encoding.UTF8, "application/json");
        }

        using var response = await http.SendAsync(request, cancellationToken).ConfigureAwait(false);
        return (response.StatusCode, await response.Content.ReadAsStringAsync(cancellationToken).ConfigureAwait(false));
    }
}
