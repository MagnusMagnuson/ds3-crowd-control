using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Text.Json;

namespace Ds3CrowdControl.Core;

public sealed class SessionSecret
{
    public string Secret { get; }

    private SessionSecret(string secret) => Secret = secret;

    public static SessionSecret Create()
    {
        var bytes = System.Security.Cryptography.RandomNumberGenerator.GetBytes(32);
        return new SessionSecret(Convert.ToHexString(bytes));
    }
}

public sealed class GameBridgeServer : IAsyncDisposable
{
    private readonly SessionSecret _session;
    private readonly object _gate = new();
    private TcpListener? _listener;
    private StreamWriter? _writer;
    private CancellationTokenSource? _cts;
    private TaskCompletionSource<EffectResult>? _pending;
    private string? _pendingId;

    public GameBridgeServer(SessionSecret session) => _session = session;

    public int Port { get; private set; }

    public GameState State { get; private set; } = GameState.Disconnected;

    public event Action? StateChanged;

    public event Action<string>? Logged;

    public event Action<IReadOnlyList<HandlerSchema>>? HandlersReceived;

    public Task StartAsync(int preferredPort = 47331, bool publishSessionFile = true, CancellationToken cancellationToken = default)
    {
        var port = preferredPort;
        TcpListener? listener = null;
        while (listener is null)
        {
            var candidate = new TcpListener(IPAddress.Loopback, port);
            try
            {
                candidate.Start();
                listener = candidate;
            }
            catch (SocketException) when (port != 0 && port < preferredPort + 20)
            {
                port++;
            }
        }

        _listener = listener;
        Port = ((IPEndPoint)listener.LocalEndpoint).Port;
        if (publishSessionFile)
        {
            AppPaths.EnsureRoot();
            var json = JsonSerializer.Serialize(new { secret = _session.Secret, port = Port });
            File.WriteAllText(AppPaths.SessionFile, json);
        }

        _cts = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
        _ = AcceptLoop(_cts.Token);
        return Task.CompletedTask;
    }

    public void Log(string message) => Logged?.Invoke(message);

    public async Task<EffectResult> TriggerAsync(string handler, JsonElement args, CancellationToken cancellationToken)
    {
        var requestId = Guid.NewGuid().ToString("N");
        var payload = JsonSerializer.Serialize(new
        {
            type = "trigger",
            requestId,
            handler,
            args
        });

        TaskCompletionSource<EffectResult> pending;
        StreamWriter writer;
        lock (_gate)
        {
            if (_writer is null)
            {
                return new EffectResult(requestId, EffectStatus.Unavailable, "the game mod is not connected");
            }

            pending = new TaskCompletionSource<EffectResult>(TaskCreationOptions.RunContinuationsAsynchronously);
            _pending = pending;
            _pendingId = requestId;
            writer = _writer;
        }

        try
        {
            await writer.WriteLineAsync(payload.AsMemory(), cancellationToken).ConfigureAwait(false);
            using var timeout = CancellationTokenSource.CreateLinkedTokenSource(cancellationToken);
            timeout.CancelAfter(TimeSpan.FromSeconds(5));
            return await pending.Task.WaitAsync(timeout.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
            return new EffectResult(requestId, EffectStatus.Retry, "the game did not answer in time");
        }
        finally
        {
            lock (_gate)
            {
                if (_pendingId == requestId)
                {
                    _pending = null;
                    _pendingId = null;
                }
            }
        }
    }

    public async ValueTask DisposeAsync()
    {
        if (_cts is not null)
        {
            await _cts.CancelAsync().ConfigureAwait(false);
            _cts.Dispose();
        }

        _listener?.Stop();
        lock (_gate)
        {
            _writer?.Dispose();
            _writer = null;
        }
    }

    private async Task AcceptLoop(CancellationToken cancellationToken)
    {
        while (!cancellationToken.IsCancellationRequested && _listener is not null)
        {
            TcpClient client;
            try
            {
                client = await _listener.AcceptTcpClientAsync(cancellationToken).ConfigureAwait(false);
            }
            catch (OperationCanceledException)
            {
                break;
            }
            catch (ObjectDisposedException)
            {
                break;
            }

            _ = Task.Run(() => ReadClient(client, cancellationToken), cancellationToken);
        }
    }

    private async Task ReadClient(TcpClient client, CancellationToken cancellationToken)
    {
        using var _ = client;
        var stream = client.GetStream();
        using var reader = new StreamReader(stream, Encoding.UTF8, false, 1024, leaveOpen: true);
        var writer = new StreamWriter(stream, new UTF8Encoding(false), 1024, leaveOpen: true) { AutoFlush = true, NewLine = "\n" };
        var authenticated = false;

        try
        {
            while (!cancellationToken.IsCancellationRequested)
            {
                var line = await reader.ReadLineAsync(cancellationToken).ConfigureAwait(false);
                if (line is null)
                {
                    break;
                }

                if (string.IsNullOrWhiteSpace(line))
                {
                    continue;
                }

                using var document = JsonDocument.Parse(line);
                var root = document.RootElement;
                var type = root.TryGetProperty("type", out var typeElement) ? typeElement.GetString() : "";
                if (!authenticated)
                {
                    if (type != "hello" || !SecretMatches(root))
                    {
                        Log("rejected a connection that did not present the session secret");
                        break;
                    }

                    authenticated = true;
                    lock (_gate)
                    {
                        _writer?.Dispose();
                        _writer = writer;
                    }

                    Log("game mod connected");
                }

                switch (type)
                {
                    case "hello":
                        ApplyHello(root);
                        break;
                    case "state":
                        ApplyState(root);
                        break;
                    case "result":
                        ApplyResult(root);
                        break;
                }
            }
        }
        catch (OperationCanceledException)
        {
        }
        catch (IOException)
        {
        }
        catch (JsonException exception)
        {
            Log($"ignored a malformed game message: {exception.Message}");
        }
        finally
        {
            lock (_gate)
            {
                if (ReferenceEquals(_writer, writer))
                {
                    _writer = null;
                    SetState(GameState.Disconnected);
                    Log("game mod detached");
                }
            }
        }
    }

    private bool SecretMatches(JsonElement root) =>
        root.TryGetProperty("secret", out var secret) && secret.GetString() == _session.Secret;

    private void ApplyHello(JsonElement root)
    {
        ApplyState(root);
        if (!root.TryGetProperty("handlers", out var handlersElement))
        {
            return;
        }

        var handlers = handlersElement.Deserialize<List<HandlerSchema>>(new JsonSerializerOptions
        {
            PropertyNameCaseInsensitive = true
        }) ?? [];
        HandlersReceived?.Invoke(handlers);
    }

    private void ApplyState(JsonElement root)
    {
        SetState(new GameState
        {
            Connected = true,
            ExeMatch = ReadBool(root, "exeMatch"),
            SessionKnown = ReadBool(root, "sessionKnown"),
            Offline = ReadBool(root, "offline"),
            InGame = ReadBool(root, "inGame"),
            CharacterName = root.TryGetProperty("characterName", out var name) ? name.GetString() ?? "" : ""
        });
    }

    private void ApplyResult(JsonElement root)
    {
        var requestId = root.TryGetProperty("requestId", out var id) ? id.GetString() ?? "" : "";
        var statusText = root.TryGetProperty("status", out var status) ? status.GetString() ?? "" : "";
        var message = root.TryGetProperty("message", out var messageElement) ? messageElement.GetString() ?? "" : "";
        var effectStatus = statusText switch
        {
            "success" => EffectStatus.Success,
            "failed" => EffectStatus.Failed,
            "retry" => EffectStatus.Retry,
            _ => EffectStatus.Unavailable
        };

        lock (_gate)
        {
            if (_pendingId == requestId)
            {
                _pending?.TrySetResult(new EffectResult(requestId, effectStatus, message));
            }
        }
    }

    private void SetState(GameState state)
    {
        State = state;
        StateChanged?.Invoke();
    }

    private static bool ReadBool(JsonElement root, string name) =>
        root.TryGetProperty(name, out var value) && value.ValueKind == JsonValueKind.True;
}
