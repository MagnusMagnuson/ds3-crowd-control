using System.Collections.ObjectModel;
using System.IO;
using System.Net.Http;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Ds3ViewerControl.Core;

namespace Ds3ViewerControl.Manager;

/// <summary>One row in the Discord page's table. Changes reach the preset straight away.</summary>
public partial class DiscordEventToggle : ObservableObject
{
    /// <summary>A week, the same ceiling as Twitch's.</summary>
    public const int MaxCooldownSeconds = 7 * 24 * 60 * 60;

    private readonly EffectPreset _preset;
    private readonly Action _changed;
    private bool _quiet;

    public DiscordEventToggle(EffectPreset preset, Action changed)
    {
        _preset = preset;
        _changed = changed;
        _onPanel = preset.DiscordEnabled;
        _cooldown = preset.DiscordCooldownSeconds;
    }

    public string Name => _preset.Name;

    [ObservableProperty]
    private bool _onPanel;

    [ObservableProperty]
    private int _cooldown;

    /// <summary>For All on and All off, which save once for everything.</summary>
    public void SetQuietly(bool value)
    {
        _quiet = true;
        OnPanel = value;
        _quiet = false;
    }

    partial void OnOnPanelChanged(bool value)
    {
        _preset.DiscordEnabled = value;
        if (!_quiet)
        {
            _changed();
        }
    }

    partial void OnCooldownChanged(int value)
    {
        var allowed = Math.Clamp(value, 0, MaxCooldownSeconds);
        if (allowed != value)
        {
            Cooldown = allowed;
            return;
        }

        _preset.DiscordCooldownSeconds = value;
        _changed();
    }
}

public partial class MainViewModel
{
    private static readonly HttpClient DiscordHttp = new();
    private static readonly TimeSpan DiscordPanelDelay = TimeSpan.FromSeconds(2);
    /// <summary>A cooldown should show on the panel quickly; presses close together still share one edit.</summary>
    private static readonly TimeSpan DiscordCooldownDelay = TimeSpan.FromSeconds(0.5);
    private Debouncer? _discordCooldownSoon;
    private System.Windows.Threading.DispatcherTimer? _discordCooldownEnds;
    private DiscordTriggerSource _discordSource = null!;
    private DiscordConnection _discord = null!;
    private Debouncer? _discordPanelSoon;

    public ObservableCollection<DiscordEventToggle> DiscordEvents { get; } = [];

    [ObservableProperty]
    private string _discordStatus = "Discord: not connected";

    [ObservableProperty]
    private string _discordButtonText = "Connect Discord";

    [ObservableProperty]
    private string _discordTokenState = "No bot token saved yet.";

    [ObservableProperty]
    private string _discordChannelId = "";

    [ObservableProperty]
    private int _discordViewerCooldown;

    private void InitDiscord()
    {
        void LogFromDiscord(string message) => Post(() => AddLog(message));
        _discordSource = new DiscordTriggerSource(_catalog, LogFromDiscord)
        {
            Paused = _effectsPaused,
            WaitingLimit = _waitingLimit,
            ViewerCooldownSeconds = _settings.DiscordViewerCooldownSeconds,
            Allowed = preset => !IsDiagnostic(preset)
        };
        _discordSource.Start(_queue);
        _discordSource.CooldownsChanged += () =>
            (_discordCooldownSoon ??= new Debouncer(DiscordCooldownDelay, () =>
            {
                Post(() => _ = WriteDiscordPanelAsync(online: true, quiet: true));
                return Task.CompletedTask;
            })).Request();
        _discord = new DiscordConnection(_discordSource, new DiscordTokenStore(AppPaths.DiscordTokenFile), DiscordHttp, LogFromDiscord);
        _discord.StatusChanged += status => Post(() => ApplyDiscordStatus(status));
        _discordChannelId = _settings.DiscordChannelId ?? "";
        _discordViewerCooldown = _settings.DiscordViewerCooldownSeconds;
        ShowDiscordTokenState();
        RebuildDiscordEvents();
        if (_discord.HasToken)
        {
            _ = ConnectDiscordAsync();
        }
    }

    private void ShowDiscordTokenState() =>
        DiscordTokenState = _discord.HasToken ? "A bot token is saved." : "No bot token saved yet.";

    private void ApplyDiscordStatus(string status)
    {
        DiscordStatus = status;
        DiscordButtonText = _discord.Connected ? "Disconnect Discord" : "Connect Discord";
    }

    private void RebuildDiscordEvents()
    {
        DiscordEvents.Clear();
        foreach (var preset in _catalog.Presets.Where(preset => !IsDiagnostic(preset)))
        {
            DiscordEvents.Add(new DiscordEventToggle(preset, DiscordEventsChanged));
        }
    }

    private void DiscordEventsChanged()
    {
        CatalogStore.Save(_catalog);
        RequestDiscordPanel();
    }

    /// <summary>Updates the panel shortly after the last change, if there is one to update.</summary>
    private void RequestDiscordPanel()
    {
        if (!_discord.Connected || _settings.DiscordPanelMessageIds.Count == 0)
        {
            return;
        }

        (_discordPanelSoon ??= new Debouncer(DiscordPanelDelay, () =>
        {
            Post(() => _ = WriteDiscordPanelAsync(online: true, quiet: true));
            return Task.CompletedTask;
        })).Request();
    }

    partial void OnDiscordChannelIdChanged(string value)
    {
        var channel = new string((value ?? "").Where(char.IsAsciiDigit).ToArray());
        if (channel != value)
        {
            DiscordChannelId = channel;
            return;
        }

        if (channel != _settings.DiscordChannelId)
        {
            // A panel in another channel is a new panel.
            _settings.DiscordPanelMessageIds = [];
        }

        _settings.DiscordChannelId = channel;
        SaveSettings();
    }

    partial void OnDiscordViewerCooldownChanged(int value)
    {
        var allowed = Math.Clamp(value, 0, DiscordEventToggle.MaxCooldownSeconds);
        if (allowed != value)
        {
            DiscordViewerCooldown = allowed;
            return;
        }

        _settings.DiscordViewerCooldownSeconds = value;
        _discordSource.ViewerCooldownSeconds = value;
        SaveSettings();
    }

    /// <summary>Called from the window, since a password box hands its text over only on request.</summary>
    public void SaveDiscordToken(string token)
    {
        if (string.IsNullOrWhiteSpace(token))
        {
            AddLog("Discord: paste the bot token first");
            return;
        }

        try
        {
            _discord.SaveToken(token);
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException or System.Security.Cryptography.CryptographicException)
        {
            AddLog($"Discord: could not save the token: {exception.Message}");
            return;
        }

        ShowDiscordTokenState();
        AddLog("Discord: bot token saved");
        _ = ConnectDiscordAsync();
    }

    [RelayCommand]
    private async Task ForgetDiscordTokenAsync()
    {
        // The buttons go grey first, while the bot can still edit them.
        await WriteDiscordPanelAsync(online: false, quiet: true);
        _discord.ForgetToken();
        ShowDiscordTokenState();
        ApplyDiscordStatus("Discord: no bot token saved");
        AddLog("Discord: bot token removed");
    }

    [RelayCommand]
    private async Task ToggleDiscordAsync()
    {
        if (_discord.Connected)
        {
            await WriteDiscordPanelAsync(online: false, quiet: true);
            _discord.Disconnect();
            return;
        }

        await ConnectDiscordAsync();
    }

    private async Task ConnectDiscordAsync()
    {
        try
        {
            await _discord.ConnectAsync(CancellationToken.None);
        }
        catch (Exception exception) when (exception is DiscordException or HttpRequestException or TaskCanceledException)
        {
            ApplyDiscordStatus("Discord: not connected");
            AddLog($"Discord: {exception.Message}");
            return;
        }

        ApplyDiscordStatus(DiscordStatus);
        // Buttons from last time come back to life.
        if (_settings.DiscordPanelMessageIds.Count > 0)
        {
            await WriteDiscordPanelAsync(online: true, quiet: true);
        }
    }

    [RelayCommand]
    private Task PostDiscordPanelAsync() => WriteDiscordPanelAsync(online: true, quiet: false);

    private async Task WriteDiscordPanelAsync(bool online, bool quiet)
    {
        if (!_discord.Connected)
        {
            if (!quiet)
            {
                AddLog("Discord: connect the bot first");
            }

            return;
        }

        try
        {
            var presets = _discordSource.PanelPresets();
            var cooldowns = _discordSource.EventCooldowns();
            var ids = await _discord.UpdatePanelAsync(_settings.DiscordChannelId, _settings.DiscordPanelMessageIds, presets, online, CancellationToken.None, cooldowns);
            if (!ids.SequenceEqual(_settings.DiscordPanelMessageIds))
            {
                _settings.DiscordPanelMessageIds = ids;
                SaveSettings();
            }

            ScheduleCooldownEnd(online ? cooldowns : new Dictionary<string, DateTimeOffset>());
            if (!quiet)
            {
                AddLog($"Discord: panel updated with {presets.Count} effects");
            }
        }
        catch (Exception exception) when (exception is DiscordException or HttpRequestException or TaskCanceledException)
        {
            AddLog($"Discord: the panel could not be updated: {exception.Message}");
        }
    }

    /// <summary>Makes the panel's buttons pressable again when the next cooldown runs out.</summary>
    private void ScheduleCooldownEnd(IReadOnlyDictionary<string, DateTimeOffset> cooldowns)
    {
        _discordCooldownEnds?.Stop();
        if (cooldowns.Count == 0)
        {
            return;
        }

        var wait = cooldowns.Values.Min() - DateTimeOffset.UtcNow + TimeSpan.FromSeconds(0.5);
        _discordCooldownEnds ??= new System.Windows.Threading.DispatcherTimer();
        _discordCooldownEnds.Interval = wait > TimeSpan.Zero ? wait : TimeSpan.FromSeconds(0.5);
        _discordCooldownEnds.Tick -= OnDiscordCooldownEnd;
        _discordCooldownEnds.Tick += OnDiscordCooldownEnd;
        _discordCooldownEnds.Start();
    }

    private void OnDiscordCooldownEnd(object? sender, EventArgs e)
    {
        _discordCooldownEnds?.Stop();
        _ = WriteDiscordPanelAsync(online: true, quiet: true);
    }

    [RelayCommand]
    private void DiscordAllOn() => SetAllDiscordEvents(true);

    [RelayCommand]
    private void DiscordAllOff() => SetAllDiscordEvents(false);

    private void SetAllDiscordEvents(bool on)
    {
        foreach (var row in DiscordEvents)
        {
            row.SetQuietly(on);
        }

        DiscordEventsChanged();
    }

    /// <summary>Called as the window closes: the buttons turn grey and say the manager is offline.</summary>
    public void SetDiscordOffline()
    {
        if (!_discord.Connected || _settings.DiscordPanelMessageIds.Count == 0)
        {
            return;
        }

        var presets = _discordSource.PanelPresets();
        var channel = _settings.DiscordChannelId;
        var ids = _settings.DiscordPanelMessageIds.ToList();
        try
        {
            Task.Run(() => _discord.UpdatePanelAsync(channel, ids, presets, online: false, CancellationToken.None)).Wait(TimeSpan.FromSeconds(3));
        }
        catch (AggregateException)
        {
            // Closing goes ahead either way; the buttons then just fail when pressed.
        }

        _discord.Disconnect();
    }
}
