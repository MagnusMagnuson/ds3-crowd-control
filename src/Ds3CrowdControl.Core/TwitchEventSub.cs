using System.Net.WebSockets;
using System.Text;
using System.Text.Json;

namespace Ds3CrowdControl.Core;

/// <summary>
/// Twitch's live event feed. A lost connection needs new subscriptions, while a reconnect
/// message moves them across, so only the first welcome on a connection chain subscribes.
/// </summary>
public sealed class TwitchEventSubClient(
    Uri url,
    Func<string, CancellationToken, Task> subscribe,
    Action<string> log)
{
    private const int DefaultKeepaliveSeconds = 30;
    private const int RememberedMessages = 500;
    private static readonly TimeSpan KeepaliveGrace = TimeSpan.FromSeconds(5);
    private static readonly TimeSpan MaxRetryDelay = TimeSpan.FromSeconds(60);

    private readonly object _gate = new();
    private readonly HashSet<string> _seen = [];
    private readonly Queue<string> _seenOrder = new();
    private ClientWebSocket? _active;

    public TimeSpan FirstRetryDelay { get; init; } = TimeSpan.FromSeconds(1);

    /// <summary>The subscription type and the event. Twitch can deliver an event twice, and this removes repeats.</summary>
    public event Action<string, JsonElement>? Notification;

    public event Action? Connected;

    public event Action? Disconnected;

    public async Task RunAsync(CancellationToken cancellationToken)
    {
        var delay = FirstRetryDelay;
        while (!cancellationToken.IsCancellationRequested)
        {
            try
            {
                await PumpAsync(url, resuming: false, () => delay = FirstRetryDelay, cancellationToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
            {
                return;
            }
            catch (Exception exception) when (!IsPermanent(exception))
            {
                log($"Twitch connection lost: {exception.Message}");
            }

            Disconnected?.Invoke();
            await Task.Delay(delay, cancellationToken).ConfigureAwait(false);
            delay = TimeSpan.FromTicks(Math.Min(delay.Ticks * 2, MaxRetryDelay.Ticks));
        }
    }

    /// <summary>Retrying cannot fix a refused sign-in or a channel without channel points.</summary>
    private static bool IsPermanent(Exception exception) =>
        exception is TwitchException
        {
            Status: System.Net.HttpStatusCode.BadRequest
                or System.Net.HttpStatusCode.Unauthorized
                or System.Net.HttpStatusCode.Forbidden
        };

    private async Task PumpAsync(Uri address, bool resuming, Action welcomed, CancellationToken cancellationToken)
    {
        using var socket = new ClientWebSocket();
        await socket.ConnectAsync(address, cancellationToken).ConfigureAwait(false);
        var keepalive = TimeSpan.FromSeconds(DefaultKeepaliveSeconds);
        Task? successor = null;
        while (true)
        {
            var text = await ReceiveAsync(socket, keepalive + KeepaliveGrace, cancellationToken).ConfigureAwait(false);
            if (text is null)
            {
                break;
            }

            using var document = JsonDocument.Parse(text);
            var metadata = document.RootElement.GetProperty("metadata");
            var payload = document.RootElement.GetProperty("payload");
            switch (metadata.GetProperty("message_type").GetString())
            {
                case "session_welcome":
                    var session = payload.GetProperty("session");
                    if (session.TryGetProperty("keepalive_timeout_seconds", out var seconds)
                        && seconds.ValueKind == JsonValueKind.Number)
                    {
                        keepalive = TimeSpan.FromSeconds(seconds.GetInt32());
                    }

                    if (!resuming)
                    {
                        await subscribe(session.GetProperty("id").GetString()!, cancellationToken).ConfigureAwait(false);
                    }

                    lock (_gate)
                    {
                        _active = socket;
                    }

                    welcomed();
                    Connected?.Invoke();
                    break;
                case "notification":
                    if (FirstDelivery(metadata.GetProperty("message_id").GetString()!))
                    {
                        Notification?.Invoke(
                            payload.GetProperty("subscription").GetProperty("type").GetString()!,
                            payload.GetProperty("event").Clone());
                    }

                    break;
                case "session_reconnect":
                    var next = new Uri(payload.GetProperty("session").GetProperty("reconnect_url").GetString()!);
                    successor = PumpAsync(next, resuming: true, welcomed, cancellationToken);
                    break;
                case "revocation":
                    log($"Twitch stopped a subscription: {payload.GetProperty("subscription").GetProperty("status").GetString()}");
                    break;
            }
        }

        bool replaced;
        lock (_gate)
        {
            replaced = !ReferenceEquals(_active, socket);
        }

        if (replaced && successor is not null)
        {
            await successor.ConfigureAwait(false);
            return;
        }

        throw new IOException("the connection closed");
    }

    private bool FirstDelivery(string messageId)
    {
        lock (_gate)
        {
            if (!_seen.Add(messageId))
            {
                return false;
            }

            _seenOrder.Enqueue(messageId);
            if (_seenOrder.Count > RememberedMessages)
            {
                _seen.Remove(_seenOrder.Dequeue());
            }

            return true;
        }
    }

    /// <summary>Returns null when the socket closes or Twitch stays silent past its keepalive.</summary>
    private static async Task<string?> ReceiveAsync(ClientWebSocket socket, TimeSpan timeout, CancellationToken cancellationToken)
    {
        using var timer = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        timer.CancelAfter(timeout);
        var buffer = new byte[16 * 1024];
        using var message = new MemoryStream();
        try
        {
            while (true)
            {
                var result = await socket.ReceiveAsync(buffer, timer.Token).ConfigureAwait(false);
                if (result.MessageType == WebSocketMessageType.Close)
                {
                    return null;
                }

                message.Write(buffer, 0, result.Count);
                if (result.EndOfMessage)
                {
                    return Encoding.UTF8.GetString(message.GetBuffer(), 0, (int)message.Length);
                }
            }
        }
        catch (OperationCanceledException) when (!cancellationToken.IsCancellationRequested)
        {
            return null;
        }
    }
}
