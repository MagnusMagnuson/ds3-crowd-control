using System.Net;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;

namespace Ds3CrowdControl.Core;

public static class TwitchConfig
{
    /// <summary>A public client, so this id is not a secret.</summary>
    public const string ClientId = "18lmnmypqi9nmzcb9x8ufkzanhllu9";

    /// <summary>Bits and subs are asked for now so adding them later needs no new sign-in.</summary>
    public const string Scopes = "channel:manage:redemptions bits:read channel:read:subscriptions";

    public static readonly Uri IdUrl = new("https://id.twitch.tv");
    public static readonly Uri ApiUrl = new("https://api.twitch.tv");
    public static readonly Uri EventSubUrl = new("wss://eventsub.wss.twitch.tv/ws?keepalive_timeout_seconds=30");

    /// <summary>Where <c>twitch event websocket start-server</c> listens by default.</summary>
    public static readonly Uri TestServerUrl = new("ws://127.0.0.1:8080/ws");
}

public sealed class TwitchException(string message, HttpStatusCode? status = null) : Exception(message)
{
    public HttpStatusCode? Status { get; } = status;
}

public sealed record TwitchDeviceCode(
    string DeviceCode,
    string UserCode,
    string VerificationUri,
    int ExpiresInSeconds,
    int IntervalSeconds);

public sealed class TwitchTokens
{
    public string AccessToken { get; set; } = "";
    public string RefreshToken { get; set; } = "";
    public string UserId { get; set; } = "";
    public string Login { get; set; } = "";
}

/// <summary>Keeps tokens encrypted for the current Windows user.</summary>
public sealed class TwitchTokenStore(string path)
{
    public TwitchTokens? Load()
    {
        if (!File.Exists(path))
        {
            return null;
        }

        try
        {
            var bytes = ProtectedData.Unprotect(File.ReadAllBytes(path), null, DataProtectionScope.CurrentUser);
            return JsonSerializer.Deserialize<TwitchTokens>(Encoding.UTF8.GetString(bytes));
        }
        catch (Exception exception) when (exception is CryptographicException or JsonException)
        {
            return null;
        }
    }

    public void Save(TwitchTokens tokens)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        var bytes = Encoding.UTF8.GetBytes(JsonSerializer.Serialize(tokens));
        File.WriteAllBytes(path, ProtectedData.Protect(bytes, null, DataProtectionScope.CurrentUser));
    }

    public void Delete() => File.Delete(path);
}

/// <summary>The device code sign-in, which needs no client secret and no local web server.</summary>
public sealed class TwitchAuth(HttpClient http, Uri idUrl, string clientId)
{
    public async Task<TwitchDeviceCode> StartDeviceAsync(CancellationToken cancellationToken)
    {
        var (status, body) = await PostAsync("oauth2/device", new Dictionary<string, string>
        {
            ["client_id"] = clientId,
            ["scopes"] = TwitchConfig.Scopes
        }, cancellationToken).ConfigureAwait(false);
        if (status != HttpStatusCode.OK)
        {
            throw new TwitchException($"Twitch refused the sign-in: {ErrorMessage(body)}", status);
        }

        using var document = JsonDocument.Parse(body);
        var root = document.RootElement;
        return new TwitchDeviceCode(
            root.GetProperty("device_code").GetString()!,
            root.GetProperty("user_code").GetString()!,
            root.GetProperty("verification_uri").GetString()!,
            root.GetProperty("expires_in").GetInt32(),
            root.GetProperty("interval").GetInt32());
    }

    /// <summary>Waits for the person to approve the code on Twitch.</summary>
    public async Task<TwitchTokens> WaitForApprovalAsync(TwitchDeviceCode device, CancellationToken cancellationToken)
    {
        var deadline = DateTime.UtcNow.AddSeconds(device.ExpiresInSeconds);
        var interval = TimeSpan.FromSeconds(Math.Max(device.IntervalSeconds, 0));
        while (DateTime.UtcNow < deadline)
        {
            await Task.Delay(interval, cancellationToken).ConfigureAwait(false);
            var (status, body) = await PostAsync("oauth2/token", new Dictionary<string, string>
            {
                ["client_id"] = clientId,
                ["scopes"] = TwitchConfig.Scopes,
                ["device_code"] = device.DeviceCode,
                ["grant_type"] = "urn:ietf:params:oauth:grant-type:device_code"
            }, cancellationToken).ConfigureAwait(false);
            if (status == HttpStatusCode.OK)
            {
                return await WithIdentityAsync(ReadTokens(body), cancellationToken).ConfigureAwait(false);
            }

            var error = ErrorMessage(body);
            if (error.Contains("slow_down", StringComparison.OrdinalIgnoreCase))
            {
                interval += TimeSpan.FromSeconds(5);
            }
            else if (!error.Contains("authorization_pending", StringComparison.OrdinalIgnoreCase))
            {
                throw new TwitchException($"Twitch sign-in failed: {error}", status);
            }
        }

        throw new TwitchException("the Twitch sign-in code expired");
    }

    /// <summary>Each refresh token works once, so the result must replace the saved tokens.</summary>
    public async Task<TwitchTokens> RefreshAsync(TwitchTokens tokens, CancellationToken cancellationToken)
    {
        var (status, body) = await PostAsync("oauth2/token", new Dictionary<string, string>
        {
            ["client_id"] = clientId,
            ["grant_type"] = "refresh_token",
            ["refresh_token"] = tokens.RefreshToken
        }, cancellationToken).ConfigureAwait(false);
        if (status != HttpStatusCode.OK)
        {
            throw new TwitchException($"the Twitch sign-in has expired: {ErrorMessage(body)}", status);
        }

        var refreshed = ReadTokens(body);
        refreshed.UserId = tokens.UserId;
        refreshed.Login = tokens.Login;
        return refreshed;
    }

    /// <summary>
    /// Asks Twitch to cancel the sign-in, so the app no longer counts as connected to the
    /// channel. An access token runs out after a few hours and Twitch refuses to cancel an old
    /// one, so in that case a fresh one is fetched first and cancelled instead.
    /// </summary>
    public async Task<bool> RevokeAsync(TwitchTokens tokens, CancellationToken cancellationToken)
    {
        if (await RevokeAccessTokenAsync(tokens.AccessToken, cancellationToken).ConfigureAwait(false))
        {
            return true;
        }

        if (string.IsNullOrEmpty(tokens.RefreshToken))
        {
            return false;
        }

        TwitchTokens fresh;
        try
        {
            fresh = await RefreshAsync(tokens, cancellationToken).ConfigureAwait(false);
        }
        catch (TwitchException)
        {
            // The refresh token no longer works either, so nothing is left to cancel.
            return true;
        }

        return await RevokeAccessTokenAsync(fresh.AccessToken, cancellationToken).ConfigureAwait(false);
    }

    private async Task<bool> RevokeAccessTokenAsync(string accessToken, CancellationToken cancellationToken)
    {
        if (string.IsNullOrEmpty(accessToken))
        {
            return false;
        }

        var (status, _) = await PostAsync("oauth2/revoke", new Dictionary<string, string>
        {
            ["client_id"] = clientId,
            ["token"] = accessToken
        }, cancellationToken).ConfigureAwait(false);
        return status == HttpStatusCode.OK;
    }

    /// <summary>Twitch expects apps that keep a sign-in to validate it.</summary>
    public async Task<bool> ValidateAsync(string accessToken, CancellationToken cancellationToken)
    {
        using var request = ValidateRequest(accessToken);
        using var response = await http.SendAsync(request, cancellationToken).ConfigureAwait(false);
        return response.IsSuccessStatusCode;
    }

    private async Task<TwitchTokens> WithIdentityAsync(TwitchTokens tokens, CancellationToken cancellationToken)
    {
        using var request = ValidateRequest(tokens.AccessToken);
        using var response = await http.SendAsync(request, cancellationToken).ConfigureAwait(false);
        var body = await response.Content.ReadAsStringAsync(cancellationToken).ConfigureAwait(false);
        if (!response.IsSuccessStatusCode)
        {
            throw new TwitchException($"Twitch did not accept the new sign-in: {ErrorMessage(body)}", response.StatusCode);
        }

        using var document = JsonDocument.Parse(body);
        tokens.UserId = document.RootElement.GetProperty("user_id").GetString()!;
        tokens.Login = document.RootElement.GetProperty("login").GetString()!;
        return tokens;
    }

    private HttpRequestMessage ValidateRequest(string accessToken)
    {
        var request = new HttpRequestMessage(HttpMethod.Get, new Uri(idUrl, "oauth2/validate"));
        request.Headers.TryAddWithoutValidation("Authorization", $"OAuth {accessToken}");
        return request;
    }

    private async Task<(HttpStatusCode Status, string Body)> PostAsync(
        string path,
        Dictionary<string, string> form,
        CancellationToken cancellationToken)
    {
        using var content = new FormUrlEncodedContent(form);
        using var response = await http.PostAsync(new Uri(idUrl, path), content, cancellationToken).ConfigureAwait(false);
        return (response.StatusCode, await response.Content.ReadAsStringAsync(cancellationToken).ConfigureAwait(false));
    }

    private static TwitchTokens ReadTokens(string body)
    {
        using var document = JsonDocument.Parse(body);
        return new TwitchTokens
        {
            AccessToken = document.RootElement.GetProperty("access_token").GetString()!,
            RefreshToken = document.RootElement.GetProperty("refresh_token").GetString()!
        };
    }

    internal static string ErrorMessage(string body)
    {
        try
        {
            using var document = JsonDocument.Parse(body);
            return document.RootElement.TryGetProperty("message", out var message)
                ? message.GetString() ?? body
                : body;
        }
        catch (JsonException)
        {
            return body;
        }
    }
}
