using System.Text.Json;
using System.Text.Json.Nodes;

namespace Ds3ViewerControl.Core;

/// <summary>
/// Button presses on the Discord panel. Pressing is free, so cooldowns are the only brake:
/// one per event, shared by everyone, and one per viewer across all buttons. A press that
/// fails gives both back.
///
/// A press on the public panel answers with the viewer's personal panel: a message only
/// they see, with every button, grey while their own cooldown or the event's runs. A press
/// there changes that same message, so each viewer has one personal panel at a time.
/// </summary>
public sealed class DiscordTriggerSource(Catalog catalog, Action<string> log) : ITriggerSource
{
    public const string SourcePrefix = "discord:";
    private const int ButtonPress = 3;
    private const int MaxViewerNameLength = 32;
    public const string Welcome = "Your own panel. Grey buttons are on cooldown.";
    /// <summary>
    /// Discord lets a bot change or remove its answer for 15 minutes after a press. The
    /// personal panel is removed a little before, so nobody is left with one that cannot
    /// follow the cooldowns anymore.
    /// </summary>
    public static readonly TimeSpan PanelLifetime = TimeSpan.FromMinutes(14);
    private static readonly TimeSpan RefreshMargin = TimeSpan.FromSeconds(0.5);

    private readonly object _gate = new();
    private readonly Dictionary<string, DateTimeOffset> _eventReady = [];
    private readonly Dictionary<string, DateTimeOffset> _viewerReady = [];
    private readonly Dictionary<string, PersonalPanel> _panels = [];
    private int _waiting;

    /// <summary>What the bot knows about one viewer's personal panel.</summary>
    private sealed class PersonalPanel
    {
        /// <summary>The latest press on it, which is what can change it.</summary>
        public string Token = "";
        public DateTimeOffset At;
        public int Page;
        public string Status = Welcome;
        /// <summary>Counts up with every press, so an older scheduled refresh knows to stop.</summary>
        public int Generation;
    }

    public string Name => "discord";

    public IDiscordRest? Rest { get; set; }

    /// <summary>Needed to change an answer after the press.</summary>
    public string ApplicationId { get; set; } = "";

    public Func<string, string, CancellationToken, Task<EffectResult>>? RunEffect { get; set; }

    /// <summary>How long one viewer waits after pressing any button.</summary>
    public int ViewerCooldownSeconds { get; set; } = 30;

    /// <summary>More than this waiting at once are turned away. Zero means no limit.</summary>
    public int WaitingLimit { get; set; }

    public bool Paused { get; set; }

    /// <summary>Events that may never get a button, such as the testing tools.</summary>
    public Func<EffectPreset, bool> Allowed { get; set; } = _ => true;

    public Func<DateTimeOffset> Now { get; init; } = () => DateTimeOffset.UtcNow;

    /// <summary>For tests, which cannot wait minutes for a refresh.</summary>
    public Func<TimeSpan, CancellationToken, Task> Delay { get; init; } = Task.Delay;

    /// <summary>An event's cooldown started or was given back, so the public panel should show it.</summary>
    public event Action? CooldownsChanged;

    /// <summary>The events on cooldown right now, with the time each is ready again.</summary>
    public IReadOnlyDictionary<string, DateTimeOffset> EventCooldowns()
    {
        var now = Now();
        lock (_gate)
        {
            return _eventReady.Where(pair => pair.Value > now).ToDictionary(pair => pair.Key, pair => pair.Value);
        }
    }

    public void Start(EffectQueue queue) => RunEffect = queue.EnqueueAsync;

    public void Stop() => RunEffect = null;

    /// <summary>The events shown on the panels, in the catalog's order.</summary>
    public IReadOnlyList<EffectPreset> PanelPresets() =>
        catalog.Presets.Where(preset => preset.DiscordEnabled && Allowed(preset)).ToList();

    /// <summary>Handles one interaction from the gateway. Anything that is not one of our buttons is left alone.</summary>
    public async Task HandleAsync(JsonElement interaction, CancellationToken cancellationToken)
    {
        if (!interaction.TryGetProperty("type", out var type) || type.GetInt32() != ButtonPress)
        {
            return;
        }

        var button = DiscordPanel.Parse(
            interaction.TryGetProperty("data", out var data) && data.TryGetProperty("custom_id", out var custom) ? custom.GetString() : null);
        var rest = Rest;
        if (button is null || rest is null)
        {
            return;
        }

        var interactionId = interaction.GetProperty("id").GetString() ?? "";
        var interactionToken = interaction.GetProperty("token").GetString() ?? "";
        var (viewerId, viewer) = Viewer(interaction);
        var (kind, value) = button.Value;
        var fromPersonal = kind != DiscordPanel.ButtonKind.Public;

        if (kind == DiscordPanel.ButtonKind.Page)
        {
            var page = int.TryParse(value, out var wanted) ? wanted : CurrentPage(viewerId);
            Remember(viewerId, interactionToken, page, status: null, out _);
            await rest.UpdateAsync(interactionId, interactionToken, Panel(viewerId), cancellationToken).ConfigureAwait(false);
            Schedule(viewerId, cancellationToken);
            return;
        }

        var preset = catalog.FindPreset(value);
        var (refusal, release) = Reserve(preset, viewerId);
        var answer = refusal ?? $"{preset!.Name} is queued.";
        if (refusal is null)
        {
            CooldownsChanged?.Invoke();
        }

        Remember(viewerId, interactionToken, page: null, answer, out var previousToken);
        var message = Panel(viewerId);
        if (fromPersonal)
        {
            await rest.UpdateAsync(interactionId, interactionToken, message, cancellationToken).ConfigureAwait(false);
        }
        else
        {
            await rest.ReplyAsync(interactionId, interactionToken, message, cancellationToken).ConfigureAwait(false);
            // A press on the public panel opens a new personal panel; the old one goes.
            if (previousToken is not null)
            {
                await RemoveAsync(rest, previousToken, cancellationToken).ConfigureAwait(false);
            }
        }

        Schedule(viewerId, cancellationToken);
        if (refusal is not null || preset is null)
        {
            return;
        }

        // Discord wants an answer within three seconds, so the panel says "queued" first
        // and is changed once the effect has had its turn.
        Interlocked.Increment(ref _waiting);
        try
        {
            var run = RunEffect;
            var result = run is null
                ? new EffectResult("", EffectStatus.Unavailable, "the manager is shutting down")
                : await run(preset.Id, SourcePrefix + viewer, cancellationToken).ConfigureAwait(false);
            string outcome;
            if (result.Status == EffectStatus.Success)
            {
                outcome = $"{preset.Name} ran.";
            }
            else
            {
                release();
                CooldownsChanged?.Invoke();
                outcome = $"{preset.Name} did not run: {result.Message}. Your cooldown was given back.";
                log($"Discord: {viewer}'s {preset.Name} did not run: {result.Message}");
            }

            await ShowAsync(rest, viewerId, interactionToken, outcome, cancellationToken).ConfigureAwait(false);
        }
        catch (Exception exception) when (exception is not OperationCanceledException)
        {
            release();
            CooldownsChanged?.Invoke();
            log($"Discord: {viewer}'s {preset.Name}: {exception.Message}");
        }
        finally
        {
            Interlocked.Decrement(ref _waiting);
        }
    }

    /// <summary>The viewer's personal panel as it should look right now.</summary>
    private JsonObject Panel(string viewerId)
    {
        var now = Now();
        DateTimeOffset? viewerReady;
        int page;
        string status;
        lock (_gate)
        {
            viewerReady = _viewerReady.TryGetValue(viewerId, out var ready) && ready > now ? ready : null;
            var panel = _panels.GetValueOrDefault(viewerId);
            page = panel?.Page ?? 0;
            status = panel?.Status ?? Welcome;
        }

        return DiscordPanel.Personal(PanelPresets(), page, status, viewerReady, EventCooldowns());
    }

    private int CurrentPage(string viewerId)
    {
        lock (_gate)
        {
            return _panels.GetValueOrDefault(viewerId)?.Page ?? 0;
        }
    }

    /// <summary>
    /// Notes the latest press on the viewer's personal panel, and returns the press that
    /// held an older panel, which is still there if it is young enough to be removed.
    /// </summary>
    private string? Remember(string viewerId, string token, int? page, string? status, out string? previousToken)
    {
        var now = Now();
        lock (_gate)
        {
            previousToken = _panels.TryGetValue(viewerId, out var old) && now - old.At < PanelLifetime ? old.Token : null;
            var panel = old ?? new PersonalPanel();
            panel.Token = token;
            panel.At = now;
            panel.Page = page ?? panel.Page;
            panel.Status = status ?? panel.Status;
            panel.Generation++;
            _panels[viewerId] = panel;
            foreach (var stale in _panels.Where(pair => now - pair.Value.At > PanelLifetime).Select(pair => pair.Key).ToList())
            {
                _panels.Remove(stale);
            }

            return panel.Status;
        }
    }

    /// <summary>Changes the status line, if this press still holds the viewer's panel.</summary>
    private async Task ShowAsync(IDiscordRest rest, string viewerId, string token, string status, CancellationToken cancellationToken)
    {
        lock (_gate)
        {
            if (!_panels.TryGetValue(viewerId, out var panel) || panel.Token != token)
            {
                return;
            }

            panel.Status = status;
        }

        await EditAsync(rest, viewerId, token, cancellationToken).ConfigureAwait(false);
        Schedule(viewerId, cancellationToken);
    }

    private async Task EditAsync(IDiscordRest rest, string viewerId, string token, CancellationToken cancellationToken)
    {
        try
        {
            await rest.EditResponseAsync(ApplicationId, token, Panel(viewerId), cancellationToken).ConfigureAwait(false);
        }
        catch (Exception exception) when (exception is DiscordException or HttpRequestException)
        {
            // The viewer dismissed the panel, or it is too old to change. Nothing to do.
        }
    }

    private async Task RemoveAsync(IDiscordRest rest, string token, CancellationToken cancellationToken)
    {
        if (ApplicationId.Length == 0)
        {
            return;
        }

        try
        {
            await rest.DeleteResponseAsync(ApplicationId, token, cancellationToken).ConfigureAwait(false);
        }
        catch (Exception exception) when (exception is DiscordException or HttpRequestException)
        {
        }
    }

    /// <summary>
    /// Refreshes the viewer's personal panel when the next cooldown on it runs out, so its
    /// buttons come back, and removes the panel before Discord stops allowing changes.
    /// </summary>
    private void Schedule(string viewerId, CancellationToken cancellationToken)
    {
        var rest = Rest;
        string token;
        int generation;
        DateTimeOffset at;
        var now = Now();
        DateTimeOffset? next;
        lock (_gate)
        {
            if (rest is null || !_panels.TryGetValue(viewerId, out var panel))
            {
                return;
            }

            token = panel.Token;
            generation = panel.Generation;
            at = panel.At;
            var ends = _eventReady.Values.Append(_viewerReady.GetValueOrDefault(viewerId)).Where(end => end > now).ToList();
            next = ends.Count == 0 ? null : ends.Min();
        }

        bool Current()
        {
            lock (_gate)
            {
                return _panels.TryGetValue(viewerId, out var panel) && panel.Generation == generation;
            }
        }

        var removeAt = at + PanelLifetime;
        if (next is { } refreshAt && refreshAt + RefreshMargin < removeAt)
        {
            _ = Task.Run(async () =>
            {
                await Delay(refreshAt + RefreshMargin - Now(), cancellationToken).ConfigureAwait(false);
                if (Current())
                {
                    await EditAsync(rest, viewerId, token, cancellationToken).ConfigureAwait(false);
                    Schedule(viewerId, cancellationToken);
                }
            }, CancellationToken.None);
            return;
        }

        _ = Task.Run(async () =>
        {
            await Delay(removeAt - Now(), cancellationToken).ConfigureAwait(false);
            if (Current())
            {
                lock (_gate)
                {
                    _panels.Remove(viewerId);
                }

                await RemoveAsync(rest, token, cancellationToken).ConfigureAwait(false);
            }
        }, CancellationToken.None);
    }

    /// <summary>
    /// Says why a press cannot run now, or takes both cooldowns and returns how to give
    /// them back if the effect fails.
    /// </summary>
    private (string? Refusal, Action Release) Reserve(EffectPreset? preset, string viewerId)
    {
        static void Nothing() { }

        if (preset is null || !preset.DiscordEnabled || !Allowed(preset))
        {
            return ("That effect is not on the panel anymore.", Nothing);
        }

        if (RunEffect is null)
        {
            return ("DS3 Viewer Control is not running right now.", Nothing);
        }

        if (Paused)
        {
            return ("Effects are paused right now. Try again later.", Nothing);
        }

        var limit = WaitingLimit;
        if (limit > 0 && Volatile.Read(ref _waiting) >= limit)
        {
            return ("Too many effects are waiting. Try again in a moment.", Nothing);
        }

        var now = Now();
        lock (_gate)
        {
            if (_eventReady.TryGetValue(preset.Id, out var eventReady) && eventReady > now)
            {
                return ($"{preset.Name} is ready again in {Seconds(eventReady - now)}.", Nothing);
            }

            if (_viewerReady.TryGetValue(viewerId, out var viewerReady) && viewerReady > now)
            {
                return ($"You can send another effect in {Seconds(viewerReady - now)}.", Nothing);
            }

            var hadEvent = _eventReady.TryGetValue(preset.Id, out var oldEvent);
            var hadViewer = _viewerReady.TryGetValue(viewerId, out var oldViewer);
            _eventReady[preset.Id] = now + TimeSpan.FromSeconds(Math.Max(0, preset.DiscordCooldownSeconds));
            _viewerReady[viewerId] = now + TimeSpan.FromSeconds(Math.Max(0, ViewerCooldownSeconds));
            return (null, () =>
            {
                lock (_gate)
                {
                    if (hadEvent) _eventReady[preset.Id] = oldEvent; else _eventReady.Remove(preset.Id);
                    if (hadViewer) _viewerReady[viewerId] = oldViewer; else _viewerReady.Remove(viewerId);
                }
            });
        }
    }

    private static string Seconds(TimeSpan left)
    {
        var seconds = (int)Math.Ceiling(left.TotalSeconds);
        return seconds == 1 ? "1 second" : $"{seconds} seconds";
    }

    /// <summary>The presser's id and the name to show: their name on that server, else their display name.</summary>
    public static (string Id, string Name) Viewer(JsonElement interaction)
    {
        var member = interaction.TryGetProperty("member", out var m) && m.ValueKind == JsonValueKind.Object ? m : default;
        var user = member.ValueKind == JsonValueKind.Object && member.TryGetProperty("user", out var mu)
            ? mu
            : interaction.TryGetProperty("user", out var u) ? u : default;
        var id = user.ValueKind == JsonValueKind.Object && user.TryGetProperty("id", out var userId) ? userId.GetString() ?? "" : "";
        var name = Text(member, "nick") ?? Text(user, "global_name") ?? Text(user, "username") ?? "Discord viewer";
        name = name.Trim();
        return (id, name.Length <= MaxViewerNameLength ? name : name[..MaxViewerNameLength]);
    }

    private static string? Text(JsonElement element, string property) =>
        element.ValueKind == JsonValueKind.Object
        && element.TryGetProperty(property, out var value)
        && value.ValueKind == JsonValueKind.String
        && !string.IsNullOrWhiteSpace(value.GetString())
            ? value.GetString()
            : null;
}
