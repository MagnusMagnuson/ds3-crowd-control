using System.Text.Json;

namespace Ds3CrowdControl.Core;

/// <summary>
/// The streamer's own Discord bot: the saved token, the live connection that delivers
/// button presses, and the panel in the chosen channel.
/// </summary>
public sealed class DiscordConnection
{
    private readonly DiscordTriggerSource _source;
    private readonly DiscordTokenStore _tokens;
    private readonly HttpClient _http;
    private readonly Action<string> _log;
    private readonly SemaphoreSlim _panelTurn = new(1, 1);
    private CancellationTokenSource? _running;
    private Task? _gateway;

    public DiscordConnection(DiscordTriggerSource source, DiscordTokenStore tokens, HttpClient http, Action<string> log)
    {
        _source = source;
        _tokens = tokens;
        _http = http;
        _log = log;
    }

    /// <summary>For tests: a stand-in for Discord's web API and its live connection.</summary>
    public Func<string, IDiscordRest>? RestFactory { get; init; }

    public Func<string, Func<JsonElement, Task>, Action<string>, CancellationToken, Task>? GatewayRunner { get; init; }

    public event Action<string>? StatusChanged;

    public bool HasToken => _tokens.Load() is not null;

    public bool Connected => _running is not null;

    public void SaveToken(string token) => _tokens.Save(token);

    public void ForgetToken()
    {
        Disconnect();
        _tokens.Delete();
        StatusChanged?.Invoke("Discord: no bot token saved");
    }

    /// <summary>Checks the token with Discord, then keeps the live connection open until <see cref="Disconnect"/>.</summary>
    public async Task ConnectAsync(CancellationToken cancellationToken)
    {
        var token = _tokens.Load() ?? throw new DiscordException("no bot token saved yet");
        Disconnect();
        var rest = RestFactory?.Invoke(token) ?? new DiscordRestClient(_http, token);
        var me = await rest.GetMeAsync(cancellationToken).ConfigureAwait(false);
        _source.Rest = rest;
        _source.ApplicationId = me.Id;
        var running = new CancellationTokenSource();
        _running = running;
        StatusChanged?.Invoke($"Discord: connecting as {me.Name}");
        _gateway = Task.Run(() => RunGatewayAsync(token, me, running.Token), CancellationToken.None);
    }

    public void Disconnect()
    {
        var running = _running;
        _running = null;
        running?.Cancel();
        _source.Rest = null;
        if (running is not null)
        {
            StatusChanged?.Invoke("Discord: not connected");
        }
    }

    private async Task RunGatewayAsync(string token, DiscordBotUser me, CancellationToken cancellationToken)
    {
        Task Handle(JsonElement interaction) => _source.HandleAsync(interaction, cancellationToken);
        void Ready(string application)
        {
            if (application.Length > 0)
            {
                _source.ApplicationId = application;
            }

            StatusChanged?.Invoke($"Discord: connected as {me.Name}");
        }

        try
        {
            if (GatewayRunner is not null)
            {
                await GatewayRunner(token, Handle, Ready, cancellationToken).ConfigureAwait(false);
                return;
            }

            var gateway = new DiscordGatewayClient(token, _log);
            gateway.Ready += Ready;
            gateway.Disconnected += () =>
            {
                if (!cancellationToken.IsCancellationRequested)
                {
                    StatusChanged?.Invoke($"Discord: reconnecting as {me.Name}");
                }
            };
            // Each press is handled on its own, so a slow effect never holds up the next press.
            gateway.Interaction += interaction => _ = Task.Run(async () =>
            {
                try
                {
                    await Handle(interaction).ConfigureAwait(false);
                }
                catch (Exception exception) when (exception is not OperationCanceledException)
                {
                    _log($"Discord: a button press failed: {exception.Message}");
                }
            }, CancellationToken.None);
            await gateway.RunAsync(cancellationToken).ConfigureAwait(false);
        }
        catch (OperationCanceledException) when (cancellationToken.IsCancellationRequested)
        {
        }
        catch (DiscordException exception)
        {
            // Retrying cannot fix this, for example a token Discord refused.
            _log($"Discord: {exception.Message}");
            _running = null;
            _source.Rest = null;
            StatusChanged?.Invoke("Discord: not connected");
        }
    }

    /// <summary>
    /// Writes the panel into the channel: edits the messages it posted before, posts more
    /// if needed and removes ones no longer needed. Returns the panel's message ids.
    /// </summary>
    public async Task<List<string>> UpdatePanelAsync(string channelId, IReadOnlyList<string> previous, IReadOnlyList<EffectPreset> presets, bool online, CancellationToken cancellationToken, IReadOnlyDictionary<string, DateTimeOffset>? cooldowns = null)
    {
        var rest = _source.Rest ?? throw new DiscordException("connect the bot first");
        if (string.IsNullOrWhiteSpace(channelId))
        {
            throw new DiscordException("enter the channel id first");
        }

        await _panelTurn.WaitAsync(cancellationToken).ConfigureAwait(false);
        try
        {
            var messages = DiscordPanel.Messages(presets, online, cooldowns);
            var ids = new List<string>();
            var index = 0;
            // Edit in place for as long as the old messages are there.
            while (index < messages.Count && index < previous.Count
                && await rest.EditMessageAsync(channelId, previous[index], messages[index], cancellationToken).ConfigureAwait(false))
            {
                ids.Add(previous[index]);
                index++;
            }

            // The rest is posted anew, so the panel stays in order, and the old ones go.
            foreach (var old in previous.Skip(index))
            {
                await rest.DeleteMessageAsync(channelId, old, cancellationToken).ConfigureAwait(false);
            }

            for (; index < messages.Count; index++)
            {
                ids.Add(await rest.PostMessageAsync(channelId, messages[index], cancellationToken).ConfigureAwait(false));
            }

            return ids;
        }
        finally
        {
            _panelTurn.Release();
        }
    }
}
