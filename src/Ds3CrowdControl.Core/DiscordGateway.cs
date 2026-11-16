using System.Net.WebSockets;
using System.Text;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace Ds3CrowdControl.Core;

/// <summary>
/// Discord's live connection, which is how button presses reach a bot. The manager needs no
/// special permissions on it: button presses arrive whatever the bot listens to, so it
/// asks for nothing else.
/// </summary>
public sealed class DiscordGatewayClient(string token, Action<string> log)
{
    public static readonly Uri DefaultUrl = new("wss://gateway.discord.gg/?v=10&encoding=json");
    private const int Dispatch = 0;
    private const int Heartbeat = 1;
    private const int Identify = 2;
    private const int Reconnect = 7;
    private const int InvalidSession = 9;
    private const int Hello = 10;
    private const int HeartbeatAck = 11;
    /// <summary>Discord closes with these when retrying cannot help.</summary>
    private static readonly HashSet<int> PermanentCloseCodes = [4004, 4010, 4011, 4012, 4013, 4014];
    private static readonly TimeSpan MaxRetryDelay = TimeSpan.FromSeconds(60);

    private readonly SemaphoreSlim _sending = new(1, 1);

    public Uri Url { get; init; } = DefaultUrl;

    public TimeSpan FirstRetryDelay { get; init; } = TimeSpan.FromSeconds(1);

    /// <summary>The bot's application id, once Discord has said hello.</summary>
    public event Action<string>? Ready;

    /// <summary>A button press, as Discord sends it.</summary>
    public event Action<JsonElement>? Interaction;

    public event Action? Disconnected;

    public async Task RunAsync(CancellationToken cancellationToken)
    {
        var delay = FirstRetryDelay;
        while (!cancellationToken.IsCancellationRequested)
        {
            try
            {
                await PumpAsync(() => delay = FirstRetryDelay, cancellationToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
            {
                return;
            }
            catch (DiscordException)
            {
                Disconnected?.Invoke();
                throw;
            }
            catch (Exception exception) when (exception is WebSocketException or IOException or JsonException or InvalidOperationException)
            {
                log($"Discord connection lost: {exception.Message}");
            }

            Disconnected?.Invoke();
            await Task.Delay(delay, cancellationToken).ConfigureAwait(false);
            delay = TimeSpan.FromTicks(Math.Min(delay.Ticks * 2, MaxRetryDelay.Ticks));
        }
    }

    private async Task PumpAsync(Action connected, CancellationToken cancellationToken)
    {
        using var socket = new ClientWebSocket();
        await socket.ConnectAsync(Url, cancellationToken).ConfigureAwait(false);
        using var stop = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        long? sequence = null;
        var acknowledged = true;
        Task? beating = null;
        try
        {
            while (!stop.IsCancellationRequested)
            {
                var text = await ReceiveAsync(socket, stop.Token).ConfigureAwait(false);
                if (text is null)
                {
                    var code = (int?)socket.CloseStatus ?? 0;
                    if (PermanentCloseCodes.Contains(code))
                    {
                        throw new DiscordException(code == 4004
                            ? "Discord refused the bot token. Check it in the Developer Portal."
                            : $"Discord closed the connection for good ({code}: {socket.CloseStatusDescription}).");
                    }

                    return;
                }

                using var document = JsonDocument.Parse(text);
                var root = document.RootElement;
                var op = root.GetProperty("op").GetInt32();
                if (root.TryGetProperty("s", out var s) && s.ValueKind == JsonValueKind.Number)
                {
                    sequence = s.GetInt64();
                }

                switch (op)
                {
                    case Hello:
                        var interval = TimeSpan.FromMilliseconds(root.GetProperty("d").GetProperty("heartbeat_interval").GetDouble());
                        beating = BeatAsync(socket, interval, () => sequence, () => acknowledged, value => acknowledged = value, stop);
                        await SendAsync(socket, new JsonObject
                        {
                            ["op"] = Identify,
                            ["d"] = new JsonObject
                            {
                                ["token"] = token,
                                ["intents"] = 0,
                                ["properties"] = new JsonObject
                                {
                                    ["os"] = "windows",
                                    ["browser"] = "ds3-crowd-control",
                                    ["device"] = "ds3-crowd-control"
                                }
                            }
                        }, stop.Token).ConfigureAwait(false);
                        break;
                    case HeartbeatAck:
                        acknowledged = true;
                        break;
                    case Heartbeat:
                        await SendAsync(socket, HeartbeatMessage(sequence), stop.Token).ConfigureAwait(false);
                        break;
                    case Reconnect:
                    case InvalidSession:
                        // A fresh connection signs in again, which is all the manager needs.
                        return;
                    case Dispatch:
                        var type = root.TryGetProperty("t", out var t) ? t.GetString() : null;
                        if (type == "READY")
                        {
                            connected();
                            var data = root.GetProperty("d");
                            var application = data.TryGetProperty("application", out var app) && app.TryGetProperty("id", out var id)
                                ? id.GetString() ?? ""
                                : data.GetProperty("user").GetProperty("id").GetString() ?? "";
                            Ready?.Invoke(application);
                        }
                        else if (type == "INTERACTION_CREATE")
                        {
                            Interaction?.Invoke(root.GetProperty("d").Clone());
                        }

                        break;
                }
            }
        }
        finally
        {
            stop.Cancel();
            if (beating is not null)
            {
                try
                {
                    await beating.ConfigureAwait(false);
                }
                catch (Exception exception) when (exception is OperationCanceledException or WebSocketException or ObjectDisposedException)
                {
                }
            }
        }
    }

    /// <summary>
    /// Keeps the connection alive. A beat Discord never acknowledged means the connection is
    /// dead without saying so, and the manager starts a new one.
    /// </summary>
    private async Task BeatAsync(ClientWebSocket socket, TimeSpan interval, Func<long?> sequence, Func<bool> acknowledged, Action<bool> setAcknowledged, CancellationTokenSource stop)
    {
        // Discord asks for the first beat at a random point within the first interval.
        await Task.Delay(TimeSpan.FromMilliseconds(interval.TotalMilliseconds * Random.Shared.NextDouble()), stop.Token).ConfigureAwait(false);
        while (!stop.IsCancellationRequested)
        {
            if (!acknowledged())
            {
                log("Discord stopped answering; reconnecting");
                await socket.CloseOutputAsync(WebSocketCloseStatus.NormalClosure, "no heartbeat answer", CancellationToken.None).ConfigureAwait(false);
                stop.Cancel();
                return;
            }

            setAcknowledged(false);
            await SendAsync(socket, HeartbeatMessage(sequence()), stop.Token).ConfigureAwait(false);
            await Task.Delay(interval, stop.Token).ConfigureAwait(false);
        }
    }

    private static JsonObject HeartbeatMessage(long? sequence) =>
        new() { ["op"] = Heartbeat, ["d"] = sequence is null ? null : JsonValue.Create(sequence.Value) };

    private async Task SendAsync(ClientWebSocket socket, JsonObject message, CancellationToken cancellationToken)
    {
        var bytes = Encoding.UTF8.GetBytes(message.ToJsonString());
        await _sending.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            await socket.SendAsync(bytes, WebSocketMessageType.Text, true, cancellationToken).ConfigureAwait(false);
        }
        finally
        {
            _sending.Release();
        }
    }

    /// <summary>One whole message, or null once the other side has closed.</summary>
    private static async Task<string?> ReceiveAsync(ClientWebSocket socket, CancellationToken cancellationToken)
    {
        var buffer = new byte[16 * 1024];
        using var message = new MemoryStream();
        while (true)
        {
            var result = await socket.ReceiveAsync(buffer, cancellationToken).ConfigureAwait(false);
            if (result.MessageType == WebSocketMessageType.Close)
            {
                return null;
            }

            message.Write(buffer, 0, result.Count);
            if (result.EndOfMessage)
            {
                return Encoding.UTF8.GetString(message.ToArray());
            }
        }
    }
}
