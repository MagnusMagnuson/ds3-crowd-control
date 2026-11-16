using System.Net;
using System.Net.Http.Headers;
using System.Security.Cryptography;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace Ds3CrowdControl.Core;

public sealed class DiscordException(string message) : Exception(message);

public sealed record DiscordBotUser(string Id, string Name);

/// <summary>What the manager needs from Discord's web API.</summary>
public interface IDiscordRest
{
    Task<DiscordBotUser> GetMeAsync(CancellationToken cancellationToken);

    /// <summary>Posts a message and returns its id.</summary>
    Task<string> PostMessageAsync(string channelId, JsonObject message, CancellationToken cancellationToken);

    /// <summary>False if the message is gone, for example because someone deleted it.</summary>
    Task<bool> EditMessageAsync(string channelId, string messageId, JsonObject message, CancellationToken cancellationToken);

    Task DeleteMessageAsync(string channelId, string messageId, CancellationToken cancellationToken);

    /// <summary>Answers a button press with a new message only the presser sees.</summary>
    Task ReplyAsync(string interactionId, string interactionToken, JsonObject message, CancellationToken cancellationToken);

    /// <summary>Answers a button press by changing the message the button is on.</summary>
    Task UpdateAsync(string interactionId, string interactionToken, JsonObject message, CancellationToken cancellationToken);

    /// <summary>
    /// Changes that answer later, for example once the effect has run. For a press answered
    /// with <see cref="UpdateAsync"/>, that is the message the button is on.
    /// </summary>
    Task EditResponseAsync(string applicationId, string interactionToken, JsonObject message, CancellationToken cancellationToken);

    /// <summary>Removes that reply. Discord allows it for 15 minutes after the press.</summary>
    Task DeleteResponseAsync(string applicationId, string interactionToken, CancellationToken cancellationToken);
}

/// <summary>Discord's web API, signed in as the streamer's own bot.</summary>
public sealed class DiscordRestClient(HttpClient http, string token) : IDiscordRest
{
    public static readonly Uri DefaultBase = new("https://discord.com/api/v10/");
    /// <summary>Discord asks bots to name themselves like this.</summary>
    private const string UserAgent = "DiscordBot (https://github.com/MagnusMagnuson/ds3-crowd-control, 0.3)";
    /// <summary>A reply with this flag is shown only to the person who pressed.</summary>
    private const int Ephemeral = 64;
    private const int ChannelMessageReply = 4;
    private const int UpdateMessage = 7;
    private const int MaxRateLimitRetries = 3;

    public Uri BaseAddress { get; init; } = DefaultBase;

    public async Task<DiscordBotUser> GetMeAsync(CancellationToken cancellationToken)
    {
        using var response = await SendAsync(HttpMethod.Get, "users/@me", null, cancellationToken).ConfigureAwait(false);
        var body = await ReadAsync(response, cancellationToken).ConfigureAwait(false);
        var name = body.TryGetProperty("username", out var username) ? username.GetString() ?? "" : "";
        return new DiscordBotUser(body.GetProperty("id").GetString() ?? "", name);
    }

    public async Task<string> PostMessageAsync(string channelId, JsonObject message, CancellationToken cancellationToken)
    {
        using var response = await SendAsync(HttpMethod.Post, $"channels/{channelId}/messages", message, cancellationToken).ConfigureAwait(false);
        var body = await ReadAsync(response, cancellationToken).ConfigureAwait(false);
        return body.GetProperty("id").GetString() ?? "";
    }

    public async Task<bool> EditMessageAsync(string channelId, string messageId, JsonObject message, CancellationToken cancellationToken)
    {
        using var response = await SendAsync(HttpMethod.Patch, $"channels/{channelId}/messages/{messageId}", message, cancellationToken).ConfigureAwait(false);
        if (response.StatusCode == HttpStatusCode.NotFound)
        {
            return false;
        }

        await ReadAsync(response, cancellationToken).ConfigureAwait(false);
        return true;
    }

    public async Task DeleteMessageAsync(string channelId, string messageId, CancellationToken cancellationToken)
    {
        using var response = await SendAsync(HttpMethod.Delete, $"channels/{channelId}/messages/{messageId}", null, cancellationToken).ConfigureAwait(false);
        if (response.StatusCode != HttpStatusCode.NotFound)
        {
            await ReadAsync(response, cancellationToken).ConfigureAwait(false);
        }
    }

    public Task ReplyAsync(string interactionId, string interactionToken, JsonObject message, CancellationToken cancellationToken)
    {
        var data = (JsonObject)message.DeepClone();
        data["flags"] = Ephemeral;
        return CallbackAsync(interactionId, interactionToken, ChannelMessageReply, data, cancellationToken);
    }

    public Task UpdateAsync(string interactionId, string interactionToken, JsonObject message, CancellationToken cancellationToken) =>
        CallbackAsync(interactionId, interactionToken, UpdateMessage, (JsonObject)message.DeepClone(), cancellationToken);

    private async Task CallbackAsync(string interactionId, string interactionToken, int type, JsonObject data, CancellationToken cancellationToken)
    {
        var callback = new JsonObject { ["type"] = type, ["data"] = data };
        using var response = await SendAsync(HttpMethod.Post, $"interactions/{interactionId}/{interactionToken}/callback", callback, cancellationToken, signed: false).ConfigureAwait(false);
        await ReadAsync(response, cancellationToken).ConfigureAwait(false);
    }

    public async Task EditResponseAsync(string applicationId, string interactionToken, JsonObject message, CancellationToken cancellationToken)
    {
        using var response = await SendAsync(HttpMethod.Patch, $"webhooks/{applicationId}/{interactionToken}/messages/@original", message, cancellationToken, signed: false).ConfigureAwait(false);
        await ReadAsync(response, cancellationToken).ConfigureAwait(false);
    }

    public async Task DeleteResponseAsync(string applicationId, string interactionToken, CancellationToken cancellationToken)
    {
        using var response = await SendAsync(HttpMethod.Delete, $"webhooks/{applicationId}/{interactionToken}/messages/@original", null, cancellationToken, signed: false).ConfigureAwait(false);
        if (response.StatusCode != HttpStatusCode.NotFound)
        {
            await ReadAsync(response, cancellationToken).ConfigureAwait(false);
        }
    }

    private async Task<HttpResponseMessage> SendAsync(HttpMethod method, string path, JsonObject? body, CancellationToken cancellationToken, bool signed = true)
    {
        for (var attempt = 0; ; attempt++)
        {
            using var request = new HttpRequestMessage(method, new Uri(BaseAddress, path));
            request.Headers.TryAddWithoutValidation("User-Agent", UserAgent);
            // Interaction replies are authorised by their own token, not the bot's.
            if (signed)
            {
                request.Headers.Authorization = new AuthenticationHeaderValue("Bot", token);
            }

            if (body is not null)
            {
                request.Content = new StringContent(body.ToJsonString(), Encoding.UTF8, "application/json");
            }

            var response = await http.SendAsync(request, cancellationToken).ConfigureAwait(false);
            if (response.StatusCode != HttpStatusCode.TooManyRequests || attempt >= MaxRateLimitRetries)
            {
                return response;
            }

            var wait = await RetryAfterAsync(response, cancellationToken).ConfigureAwait(false);
            response.Dispose();
            await Task.Delay(wait, cancellationToken).ConfigureAwait(false);
        }
    }

    private static async Task<TimeSpan> RetryAfterAsync(HttpResponseMessage response, CancellationToken cancellationToken)
    {
        try
        {
            var text = await response.Content.ReadAsStringAsync(cancellationToken).ConfigureAwait(false);
            using var document = JsonDocument.Parse(text);
            if (document.RootElement.TryGetProperty("retry_after", out var seconds) && seconds.TryGetDouble(out var value))
            {
                return TimeSpan.FromSeconds(Math.Clamp(value, 0.05, 30));
            }
        }
        catch (JsonException)
        {
        }

        return TimeSpan.FromSeconds(1);
    }

    private static async Task<JsonElement> ReadAsync(HttpResponseMessage response, CancellationToken cancellationToken)
    {
        var text = await response.Content.ReadAsStringAsync(cancellationToken).ConfigureAwait(false);
        if (!response.IsSuccessStatusCode)
        {
            throw new DiscordException(response.StatusCode switch
            {
                HttpStatusCode.Unauthorized => "Discord refused the bot token. Check it in the Developer Portal.",
                HttpStatusCode.Forbidden => "the bot is not allowed to do that here. Check that it can see the channel and send messages in it.",
                HttpStatusCode.NotFound => "Discord could not find that. Check the channel id.",
                _ => $"Discord answered {(int)response.StatusCode}: {Shorten(text)}"
            });
        }

        if (string.IsNullOrWhiteSpace(text))
        {
            return default;
        }

        using var document = JsonDocument.Parse(text);
        return document.RootElement.Clone();
    }

    private static string Shorten(string text) => text.Length <= 200 ? text : text[..200];
}

/// <summary>The bot token, encrypted for the current Windows user, like the Twitch sign-in.</summary>
public sealed class DiscordTokenStore(string path)
{
    public string? Load()
    {
        if (!File.Exists(path))
        {
            return null;
        }

        try
        {
            var bytes = ProtectedData.Unprotect(File.ReadAllBytes(path), null, DataProtectionScope.CurrentUser);
            var token = Encoding.UTF8.GetString(bytes);
            return string.IsNullOrWhiteSpace(token) ? null : token;
        }
        catch (CryptographicException)
        {
            return null;
        }
    }

    public void Save(string token)
    {
        Directory.CreateDirectory(Path.GetDirectoryName(path)!);
        File.WriteAllBytes(path, ProtectedData.Protect(Encoding.UTF8.GetBytes(token.Trim()), null, DataProtectionScope.CurrentUser));
    }

    public void Delete() => File.Delete(path);
}

/// <summary>The button panel: messages with one button per event.</summary>
public static class DiscordPanel
{
    /// <summary>Every button the manager makes starts with this, followed by the preset's id.</summary>
    public const string ButtonPrefix = "ds3cc:";
    public const int ButtonsPerRow = 5;
    public const int RowsPerMessage = 5;
    public const int ButtonsPerMessage = ButtonsPerRow * RowsPerMessage;
    private const int MaxLabelLength = 80;
    private const int ActionRow = 1;
    private const int Button = 2;
    private const int PrimaryStyle = 1;
    public const string Heading = "**Dark Souls III Crowd Control**\nPress a button to send that effect into the game. You also get your own panel, which only you can see and which shows your cooldowns.";
    public const string MoreHeading = "More effects:";
    public const string EmptyText = "**Dark Souls III Crowd Control**\nNo effects are on the panel right now.";
    public const string OfflineText = "**Dark Souls III Crowd Control**\nOffline right now. The buttons come back when the streamer starts the manager.";

    /// <summary>
    /// A time Discord shows as a live countdown, such as "in 2 minutes", on every viewer's
    /// screen without the message being edited.
    /// </summary>
    public static string Countdown(DateTimeOffset time) => $"<t:{time.ToUnixTimeSeconds()}:R>";

    /// <summary>
    /// The messages that make up the panel, in order. There is always at least one. Events
    /// on cooldown get a grey button and a line with a live countdown.
    /// </summary>
    public static IReadOnlyList<JsonObject> Messages(IReadOnlyList<EffectPreset> presets, bool online, IReadOnlyDictionary<string, DateTimeOffset>? cooldowns = null)
    {
        cooldowns ??= new Dictionary<string, DateTimeOffset>();
        if (presets.Count == 0)
        {
            return [new JsonObject { ["content"] = EmptyText, ["components"] = new JsonArray() }];
        }

        var messages = new List<JsonObject>();
        for (var start = 0; start < presets.Count; start += ButtonsPerMessage)
        {
            var rows = new JsonArray();
            var page = presets.Skip(start).Take(ButtonsPerMessage).ToList();
            for (var rowStart = 0; rowStart < page.Count; rowStart += ButtonsPerRow)
            {
                var buttons = new JsonArray();
                foreach (var preset in page.Skip(rowStart).Take(ButtonsPerRow))
                {
                    buttons.Add(new JsonObject
                    {
                        ["type"] = Button,
                        ["style"] = PrimaryStyle,
                        ["label"] = Label(preset.Name),
                        ["custom_id"] = ButtonPrefix + preset.Id,
                        ["disabled"] = !online || cooldowns.ContainsKey(preset.Id)
                    });
                }

                rows.Add(new JsonObject { ["type"] = ActionRow, ["components"] = buttons });
            }

            var heading = start == 0 ? (online ? Heading : OfflineText) : MoreHeading;
            var cooling = online
                ? page.Where(preset => cooldowns.ContainsKey(preset.Id))
                    .Select(preset => $"- {Label(preset.Name)}: ready {Countdown(cooldowns[preset.Id])}")
                    .ToList()
                : [];
            if (cooling.Count > 0)
            {
                heading += "\n**On cooldown:**\n" + string.Join("\n", cooling);
            }

            messages.Add(new JsonObject { ["content"] = heading, ["components"] = rows });
        }

        return messages;
    }

    /// <summary>A button on a viewer's personal panel, followed by the preset's id.</summary>
    public const string PersonalPrefix = ButtonPrefix + "me:";
    /// <summary>The personal panel's page buttons, followed by the page number.</summary>
    public const string PagePrefix = ButtonPrefix + "page:";
    /// <summary>With more events than fit one message, the personal panel shows this many per page.</summary>
    public const int ButtonsPerPage = 20;
    private const int SecondaryStyle = 2;

    public enum ButtonKind
    {
        Public,
        Personal,
        Page
    }

    /// <summary>What a button stands for, or null for a button this manager did not make.</summary>
    public static (ButtonKind Kind, string Value)? Parse(string? customId)
    {
        if (customId is null || !customId.StartsWith(ButtonPrefix, StringComparison.Ordinal))
        {
            return null;
        }

        foreach (var (prefix, kind) in new[] { (PersonalPrefix, ButtonKind.Personal), (PagePrefix, ButtonKind.Page) })
        {
            if (customId.StartsWith(prefix, StringComparison.Ordinal))
            {
                return customId.Length > prefix.Length ? (kind, customId[prefix.Length..]) : null;
            }
        }

        return customId.Length > ButtonPrefix.Length ? (ButtonKind.Public, customId[ButtonPrefix.Length..]) : null;
    }

    /// <summary>The preset a public panel button stands for, or null for any other button.</summary>
    public static string? PresetIdFrom(string? customId) =>
        Parse(customId) is { Kind: ButtonKind.Public } button ? button.Value : null;

    /// <summary>How many pages the personal panel has for this many events.</summary>
    public static int Pages(int count) => count <= ButtonsPerMessage ? 1 : (count + ButtonsPerPage - 1) / ButtonsPerPage;

    /// <summary>
    /// A viewer's personal panel: what happened with their last press, and the buttons,
    /// grey while either their own cooldown or the event's runs.
    /// </summary>
    public static JsonObject Personal(
        IReadOnlyList<EffectPreset> presets,
        int page,
        string status,
        DateTimeOffset? viewerReady,
        IReadOnlyDictionary<string, DateTimeOffset> cooldowns)
    {
        var pages = Pages(presets.Count);
        page = Math.Clamp(page, 0, pages - 1);
        var shown = pages == 1 ? presets.ToList() : presets.Skip(page * ButtonsPerPage).Take(ButtonsPerPage).ToList();
        var lines = new List<string> { status };
        if (viewerReady is { } ready)
        {
            lines.Add($"You can press again {Countdown(ready)}.");
        }

        var cooling = shown.Where(preset => cooldowns.ContainsKey(preset.Id))
            .Select(preset => $"- {Label(preset.Name)}: ready {Countdown(cooldowns[preset.Id])}")
            .ToList();
        if (cooling.Count > 0)
        {
            lines.Add("**On cooldown:**");
            lines.AddRange(cooling);
        }

        var rows = new JsonArray();
        for (var rowStart = 0; rowStart < shown.Count; rowStart += ButtonsPerRow)
        {
            var buttons = new JsonArray();
            foreach (var preset in shown.Skip(rowStart).Take(ButtonsPerRow))
            {
                buttons.Add(new JsonObject
                {
                    ["type"] = Button,
                    ["style"] = PrimaryStyle,
                    ["label"] = Label(preset.Name),
                    ["custom_id"] = PersonalPrefix + preset.Id,
                    ["disabled"] = viewerReady is not null || cooldowns.ContainsKey(preset.Id)
                });
            }

            rows.Add(new JsonObject { ["type"] = ActionRow, ["components"] = buttons });
        }

        if (pages > 1)
        {
            rows.Add(new JsonObject
            {
                ["type"] = ActionRow,
                ["components"] = new JsonArray
                {
                    PageButton("Previous", page - 1, page == 0),
                    PageButton($"Page {page + 1} of {pages}", page, true),
                    PageButton("Next", page + 1, page == pages - 1)
                }
            });
        }

        return new JsonObject { ["content"] = string.Join("\n", lines), ["components"] = rows };
    }

    private static JsonObject PageButton(string label, int page, bool disabled) => new()
    {
        ["type"] = Button,
        ["style"] = SecondaryStyle,
        ["label"] = label,
        // The middle button only shows where you are, but every button needs its own id.
        ["custom_id"] = PagePrefix + page + (label.StartsWith("Page", StringComparison.Ordinal) ? ":here" : ""),
        ["disabled"] = disabled
    };

    private static string Label(string name)
    {
        var label = string.IsNullOrWhiteSpace(name) ? "Effect" : name.Trim();
        return label.Length <= MaxLabelLength ? label : label[..MaxLabelLength];
    }
}
