using System.Collections.ObjectModel;
using System.Diagnostics;
using System.IO;
using System.Net.Http;
using System.Runtime.InteropServices;
using System.Text.Json;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Ds3CrowdControl.Core;

namespace Ds3CrowdControl.Manager;

public partial class ArgumentEditor : ObservableObject
{
    public ArgumentEditor(string name, string type, string value, string? label = null, string? hint = null, IReadOnlyList<string>? options = null)
    {
        Name = name;
        Type = type;
        Label = label ?? name;
        Hint = hint ?? "";
        Options = options ?? [];
        _value = value;
    }

    public string Name { get; }
    public string Type { get; }
    public string Label { get; }
    public string Hint { get; }
    public bool IsCheckBox => Type == "bool";
    public bool IsChoice => Type == "choice";
    public bool IsTextBox => !IsCheckBox && !IsChoice;
    public bool ShowsLabel => !IsCheckBox;
    public IReadOnlyList<string> Options { get; }

    public int SelectedIndex
    {
        get => int.TryParse(Value, out var index) ? index : 0;
        set => Value = value.ToString();
    }
    public bool HasHint => Hint.Length > 0;

    public bool IsChecked
    {
        get => Value == "1";
        set => Value = value ? "1" : "0";
    }

    [ObservableProperty]
    private string _value;
}

/// <summary>One row on the Events page. Changes reach the preset straight away.</summary>
public partial class EventToggle : ObservableObject
{
    public const string RedeemKind = "redeem";
    public const string BitsKind = "bits";
    public const string GiftKind = "gift";
    public const string AllKinds = "all";

    private readonly EffectPreset _preset;
    private readonly Func<bool> _synced;
    private readonly Action _changed;
    private bool _quiet;

    public EventToggle(EffectPreset preset, Func<bool> synced, Action changed)
    {
        _preset = preset;
        _synced = synced;
        _changed = changed;
        _redeem = preset.RedeemEnabled;
        _cost = preset.RedeemCost;
        _cooldown = preset.CooldownSeconds;
        _bits = preset.BitsEnabled;
        _bitsAmount = preset.BitsAmount;
        _gift = preset.GiftEnabled;
        _giftAmount = preset.GiftAmount;
    }

    public string Name => _preset.Name;

    [ObservableProperty]
    private bool _redeem;

    [ObservableProperty]
    private int _cost;

    /// <summary>Twitch allows a redeem cooldown of up to 7 days.</summary>
    public const int MaxCooldownSeconds = 7 * 24 * 60 * 60;

    [ObservableProperty]
    private int _cooldown;

    [ObservableProperty]
    private bool _bits;

    [ObservableProperty]
    private int _bitsAmount;

    [ObservableProperty]
    private bool _gift;

    [ObservableProperty]
    private int _giftAmount;

    /// <summary>For All on and All off, and for Sync, which save once for everything.</summary>
    public void SetQuietly(string kind, bool value)
    {
        var wasQuiet = _quiet;
        _quiet = true;
        if (kind is RedeemKind or AllKinds)
        {
            Redeem = value;
        }

        if (kind is BitsKind or AllKinds)
        {
            Bits = value;
        }

        if (kind is GiftKind or AllKinds)
        {
            Gift = value;
        }

        _quiet = wasQuiet;
    }

    partial void OnRedeemChanged(bool value)
    {
        _preset.RedeemEnabled = value;
        Switched(value);
    }

    partial void OnBitsChanged(bool value)
    {
        _preset.BitsEnabled = value;
        Switched(value);
    }

    partial void OnGiftChanged(bool value)
    {
        _preset.GiftEnabled = value;
        Switched(value);
    }

    partial void OnCostChanged(int value)
    {
        if (value < 1)
        {
            Cost = 1;
            return;
        }

        _preset.RedeemCost = value;
        _changed();
    }

    partial void OnCooldownChanged(int value)
    {
        var allowed = Math.Clamp(value, 0, MaxCooldownSeconds);
        if (allowed != value)
        {
            Cooldown = allowed;
            return;
        }

        _preset.CooldownSeconds = value;
        _changed();
    }

    partial void OnBitsAmountChanged(int value)
    {
        if (value < 1)
        {
            BitsAmount = 1;
            return;
        }

        _preset.BitsAmount = value;
        _changed();
    }

    partial void OnGiftAmountChanged(int value)
    {
        if (value < 1)
        {
            GiftAmount = 1;
            return;
        }

        _preset.GiftAmount = value;
        _changed();
    }

    private void Switched(bool value)
    {
        if (_quiet)
        {
            return;
        }

        if (_synced())
        {
            SetQuietly(AllKinds, value);
        }

        _changed();
    }
}

public partial class MainViewModel : ObservableObject
{
    /// <summary>Tools for working on the mod, shown on the Presets page only when asked for.</summary>
    private static readonly HashSet<string> DiagnosticHandlers = ["dump_player_data", "watch_item_button"];

    private readonly SynchronizationContext _ui;
    private readonly Catalog _catalog;
    private readonly ManagerSettings _settings;
    private readonly GameBridgeServer _bridge;
    private readonly EffectQueue _queue;
    private readonly ManualTriggerSource _manual;
    private readonly TwitchTriggerSource _twitchSource;
    private readonly TwitchConnection _twitch;
    private static readonly HttpClient TwitchHttp = new();
    private CancellationTokenSource? _twitchSignIn;

    public MainViewModel()
    {
        _ui = SynchronizationContext.Current ?? new SynchronizationContext();
        _catalog = CatalogStore.LoadOrCreate();
        _settings = ManagerSettingsStore.Load();
        _focusGameForItemTest = _settings.FocusGameForItemTest;
        _effectsPaused = _settings.EffectsPaused;
        _gapSeconds = _settings.GapSeconds;
        _waitingLimit = _settings.WaitingLimit;
        _showQueue = _settings.ShowQueue;
        _showDevTools = _settings.ShowDevTools;
        _syncTriggers = _settings.SyncTriggers;
        foreach (var preset in _catalog.Presets.Where(IsDiagnostic))
        {
            preset.RedeemEnabled = false;
            preset.BitsEnabled = false;
            preset.GiftEnabled = false;
        }

        _bridge = new GameBridgeServer(SessionSecret.Create());
        _queue = new EffectQueue(_catalog, _bridge)
        {
            Paused = _effectsPaused,
            MinGap = TimeSpan.FromSeconds(_gapSeconds),
            Trace = WriteLogFile
        };
        _queue.Changed += () => Post(RefreshQueue);
        _manual = new ManualTriggerSource();
        _manual.Start(_queue);
        void LogFromTwitch(string message) => Post(() => AddLog(message));
        _twitchSource = new TwitchTriggerSource(_catalog, LogFromTwitch)
        {
            Paused = _effectsPaused,
            WaitingLimit = _waitingLimit
        };
        _twitchSource.Start(_queue);
        _twitch = new TwitchConnection(
            _twitchSource,
            new TwitchTokenStore(AppPaths.TwitchTokensFile),
            TwitchHttp,
            LogFromTwitch);
        _twitch.StatusChanged += status => Post(() => ApplyTwitchStatus(status));
        _twitch.RewardsChanged += () => Post(() => CatalogStore.Save(_catalog));
        RebuildPresets();
        RebuildEvents();
        _bridge.Logged += message => Post(() => AddLog(message));
        _bridge.StateChanged += () => Post(ApplyState);
        _bridge.HandlersReceived += handlers => Post(() =>
        {
            var handlersChanged = !SameHandlers(_catalog.Handlers, handlers);
            if (handlersChanged)
            {
                _catalog.ReplaceHandlers(handlers);
            }

            var removed = _catalog.RemoveOrphanPresets();
            var added = new[]
            {
                _catalog.EnsurePreset("Kill player", "kill_player"),
                _catalog.EnsurePreset("Fake Death", "fake_death"),
                _catalog.EnsurePreset("Show YOU DIED", "show_you_died"),
                _catalog.EnsurePreset("Empty Stamina", "empty_stamina"),
                _catalog.EnsurePreset("No Stamina", "no_stamina"),
                _catalog.EnsurePreset("Decrease Speed", "decrease_speed"),
                _catalog.EnsurePreset("Increase Speed", "increase_speed"),
                _catalog.EnsurePreset("1 HP", "one_hp"),
                _catalog.EnsurePreset("1 Hit Run", "one_hit_run"),
                _catalog.EnsurePreset("Invincible", "invincible"),
                _catalog.EnsurePreset("Random Weapon", "random_weapon"),
                _catalog.EnsurePreset("Random Armor", "random_armor"),
                _catalog.EnsurePreset("Temporary Armor", "temporary_armor"),
                _catalog.EnsurePreset("Bombardier", "bombardier"),
                _catalog.EnsurePreset("Shuffle Stats", "shuffle_stats"),
                _catalog.EnsurePreset("Spawn Random Enemy", "spawn_random_enemy"),
                _catalog.EnsurePreset("Oversaturate", "oversaturate"),
                _catalog.EnsurePreset("Blind", "blind"),
                _catalog.EnsurePreset("Freeze", "freeze"),
                _catalog.EnsurePreset("Invert Controls", "invert_controls"),
                _catalog.EnsurePreset("Pacifist", "pacifist"),
                _catalog.EnsurePreset("Set health to 50%", "set_health_percent"),
                _catalog.EnsurePreset("Set health to random %", "set_health_random_percent"),
                _catalog.EnsurePreset("Watch item button", "watch_item_button")
            }.Where(preset => preset is not null).Cast<EffectPreset>().ToList();
            if (!handlersChanged && added.Count == 0 && removed.Count == 0)
            {
                return;
            }

            CatalogStore.Save(_catalog);
            RebuildPresets();
            RebuildEvents();
            LoadEditor();
        });
        _ = StartAsync();
        _ = _twitch.ResumeAsync();
        RefreshGearSnapshots();
        // The mod writes the snapshots, so the list is reread now and then.
        var gearTimer = new System.Windows.Threading.DispatcherTimer { Interval = TimeSpan.FromSeconds(15) };
        gearTimer.Tick += (_, _) => RefreshGearSnapshots();
        gearTimer.Start();
    }

    public ObservableCollection<EffectPreset> Presets { get; } = [];
    public ObservableCollection<string> LogLines { get; } = [];
    public ObservableCollection<ArgumentEditor> Arguments { get; } = [];
    public ObservableCollection<EventToggle> Events { get; } = [];
    public ObservableCollection<QueuedEffect> QueueRows { get; } = [];
    public ObservableCollection<GearSnapshotInfo> GearSnapshots { get; } = [];
    public bool NoGearSnapshots => GearSnapshots.Count == 0;
    public string QueueHeader => QueueRows.Count == 0 ? "Queue" : $"Queue ({QueueRows.Count})";
    public bool QueueHidden => !ShowQueue;

    [ObservableProperty]
    private EffectPreset? _selectedPreset;

    [ObservableProperty]
    private string _status = "Starting the local bridge";

    [ObservableProperty]
    private string _editName = "";

    [ObservableProperty]
    private bool _isSafe;

    [ObservableProperty]
    private bool _focusGameForItemTest;

    [ObservableProperty]
    private bool _effectsPaused;

    [ObservableProperty]
    private int _gapSeconds;

    [ObservableProperty]
    private int _waitingLimit;

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(QueueHidden))]
    private bool _showQueue;

    [ObservableProperty]
    private bool _showDevTools;

    [ObservableProperty]
    private bool _syncTriggers;

    [ObservableProperty]
    private bool _useTwitchTestServer;

    [ObservableProperty]
    private string _twitchStatus = "Twitch: not connected";

    [ObservableProperty]
    private string _twitchButtonText = "Connect Twitch";

    private string _loggedStatus = "";

    public string DllPath
    {
        get
        {
            var besideApp = Path.Combine(AppContext.BaseDirectory, "ds3mod.dll");
            var justBuilt = Path.GetFullPath(Path.Combine(
                AppContext.BaseDirectory,
                "..", "..", "..", "..", "ds3mod", "target", "release", "ds3mod.dll"));
            if (File.Exists(justBuilt)
                && (!File.Exists(besideApp) || File.GetLastWriteTimeUtc(justBuilt) > File.GetLastWriteTimeUtc(besideApp)))
            {
                return justBuilt;
            }

            return besideApp;
        }
    }

    partial void OnSelectedPresetChanged(EffectPreset? value)
    {
        LoadEditor();
        TestCommand.NotifyCanExecuteChanged();
    }

    partial void OnFocusGameForItemTestChanged(bool value)
    {
        _settings.FocusGameForItemTest = value;
        SaveSettings();
    }

    partial void OnEffectsPausedChanged(bool value)
    {
        _settings.EffectsPaused = value;
        _queue.Paused = value;
        _twitchSource.Paused = value;
        SaveSettings();
        _twitch.RequestSync();
        ApplyState();
    }

    partial void OnGapSecondsChanged(int value)
    {
        if (value < 0)
        {
            GapSeconds = 0;
            return;
        }

        _settings.GapSeconds = value;
        _queue.MinGap = TimeSpan.FromSeconds(value);
        SaveSettings();
    }

    partial void OnWaitingLimitChanged(int value)
    {
        if (value < 0)
        {
            WaitingLimit = 0;
            return;
        }

        _settings.WaitingLimit = value;
        _twitchSource.WaitingLimit = value;
        SaveSettings();
    }

    partial void OnShowQueueChanged(bool value)
    {
        _settings.ShowQueue = value;
        SaveSettings();
    }

    partial void OnShowDevToolsChanged(bool value)
    {
        _settings.ShowDevTools = value;
        SaveSettings();
        RebuildPresets();
    }

    partial void OnUseTwitchTestServerChanged(bool value)
    {
        if (value)
        {
            _twitch.StartTestServer();
        }
        else
        {
            _ = _twitch.StopTestServerAsync();
        }
    }

    partial void OnSyncTriggersChanged(bool value)
    {
        _settings.SyncTriggers = value;
        SaveSettings();
    }

    private void SaveSettings()
    {
        try
        {
            ManagerSettingsStore.Save(_settings);
        }
        catch (IOException exception)
        {
            AddLog($"Could not save manager settings: {exception.Message}");
        }
    }

    [RelayCommand]
    private async Task AttachAsync()
    {
        var (ok, message) = await Task.Run(() => GameInjector.Inject(DllPath));
        AddLog(message);
        if (!ok)
        {
            Status = message;
        }
    }

    [RelayCommand(AllowConcurrentExecutions = true)]
    private async Task ToggleTwitchAsync()
    {
        if (_twitchSignIn is not null)
        {
            _twitchSignIn.Cancel();
            return;
        }

        if (_twitch.SignedIn)
        {
            await _twitch.SignOutAsync();
            TwitchButtonText = "Connect Twitch";
            return;
        }

        using var signIn = new CancellationTokenSource();
        _twitchSignIn = signIn;
        TwitchButtonText = "Cancel";
        try
        {
            await _twitch.SignInAsync(device => Post(() => ShowTwitchCode(device)), signIn.Token);
        }
        catch (OperationCanceledException)
        {
            TwitchStatus = "Twitch: not connected";
        }
        catch (Exception exception) when (exception is TwitchException or HttpRequestException)
        {
            TwitchStatus = "Twitch: not connected";
            AddLog($"Twitch: {exception.Message}");
        }
        finally
        {
            _twitchSignIn = null;
            TwitchButtonText = _twitch.SignedIn ? "Disconnect Twitch" : "Connect Twitch";
        }
    }

    private void ShowTwitchCode(TwitchDeviceCode device)
    {
        TwitchStatus = $"Twitch: enter code {device.UserCode}";
        AddLog($"Twitch: approve the sign-in with code {device.UserCode} at {device.VerificationUri}");
        try
        {
            Process.Start(new ProcessStartInfo(device.VerificationUri) { UseShellExecute = true });
        }
        catch (System.ComponentModel.Win32Exception exception)
        {
            AddLog($"Twitch: could not open the browser: {exception.Message}");
        }
    }

    private void ApplyTwitchStatus(string status)
    {
        TwitchStatus = status;
        if (_twitchSignIn is null)
        {
            TwitchButtonText = _twitch.SignedIn ? "Disconnect Twitch" : "Connect Twitch";
        }
    }

    /// <summary>Called as the window closes, so viewers cannot redeem with nothing listening.</summary>
    public void PauseTwitchRewards()
    {
        _queue.Clear();
        Task.Run(_twitch.PauseRewardsAsync).Wait(TimeSpan.FromSeconds(3));
    }

    [RelayCommand]
    private void AllOn(string kind) => SetAllEvents(kind, true);

    [RelayCommand]
    private void AllOff(string kind) => SetAllEvents(kind, false);

    private void SetAllEvents(string kind, bool on)
    {
        foreach (var row in Events)
        {
            row.SetQuietly(SyncTriggers ? EventToggle.AllKinds : kind, on);
        }

        EventsChanged();
    }

    private void RefreshGearSnapshots()
    {
        var latest = GearSnapshotStore.Load(AppPaths.GearSnapshotsFile);
        if (latest.SequenceEqual(GearSnapshots))
        {
            return;
        }

        GearSnapshots.Clear();
        foreach (var snapshot in latest)
        {
            GearSnapshots.Add(snapshot);
        }

        OnPropertyChanged(nameof(NoGearSnapshots));
    }

    [RelayCommand(AllowConcurrentExecutions = true)]
    private async Task RestoreGearAsync(GearSnapshotInfo? snapshot)
    {
        if (snapshot is null)
        {
            return;
        }

        if (!_bridge.State.IsSafe)
        {
            AddLog("Restore gear: load your character in the game first");
            return;
        }

        var args = JsonSerializer.SerializeToElement(new { slot = snapshot.Slot });
        var result = await _bridge.TriggerAsync("restore_gear", args, CancellationToken.None);
        AddLog($"Restore gear: {result.Message}");
    }

    [RelayCommand]
    private void SaveSupportLogs()
    {
        try
        {
            var desktop = Environment.GetFolderPath(Environment.SpecialFolder.DesktopDirectory);
            var path = SupportBundle.Create(AppPaths.Root, desktop, SupportSummary(), SupportBundle.DefaultCrashDumpFolder, DateTime.Now);
            AddLog($"logs saved to {path}");
            Process.Start(new ProcessStartInfo("explorer.exe", $"/select,\"{path}\"") { UseShellExecute = true });
        }
        catch (Exception exception) when (exception is IOException or UnauthorizedAccessException or System.ComponentModel.Win32Exception)
        {
            AddLog($"could not save the logs: {exception.Message}");
        }
    }

    private string SupportSummary()
    {
        static string Stamp(string file) =>
            File.Exists(file) ? File.GetLastWriteTime(file).ToString("yyyy-MM-dd HH:mm") : "missing";
        var manager = Environment.ProcessPath ?? "";
        return string.Join(Environment.NewLine,
            $"saved: {DateTime.Now:yyyy-MM-dd HH:mm:ss}",
            $"windows: {Environment.OSVersion.VersionString}",
            $".net: {Environment.Version}",
            $"manager build: {Stamp(manager)}",
            $"mod build: {Stamp(DllPath)}",
            $"game: {Status}",
            $"twitch: {TwitchStatus}",
            $"effects paused: {EffectsPaused}, gap {GapSeconds} s, waiting limit {WaitingLimit}");
    }

    [RelayCommand]
    private void ClearQueue()
    {
        var cleared = _queue.Clear();
        AddLog($"cleared {cleared} from the queue");
    }

    private static bool IsDiagnostic(EffectPreset preset) => DiagnosticHandlers.Contains(preset.HandlerId);

    private void RebuildPresets()
    {
        var selected = SelectedPreset;
        Presets.Clear();
        foreach (var preset in _catalog.Presets.Where(preset => ShowDevTools || !IsDiagnostic(preset)))
        {
            Presets.Add(preset);
        }

        SelectedPreset = selected is null ? null : Presets.FirstOrDefault(preset => preset.Id == selected.Id);
    }

    private void RebuildEvents()
    {
        Events.Clear();
        foreach (var preset in _catalog.Presets.Where(preset => !IsDiagnostic(preset)))
        {
            Events.Add(new EventToggle(preset, () => SyncTriggers, EventsChanged));
        }
    }

    private void EventsChanged()
    {
        CatalogStore.Save(_catalog);
        _twitch.RequestSync();
    }

    private void RefreshQueue()
    {
        QueueRows.Clear();
        foreach (var effect in _queue.Snapshot())
        {
            QueueRows.Add(effect);
        }

        OnPropertyChanged(nameof(QueueHeader));
    }

    [RelayCommand(CanExecute = nameof(CanTest), AllowConcurrentExecutions = true)]
    private async Task TestAsync()
    {
        var preset = SelectedPreset;
        if (preset is null)
        {
            return;
        }

        CommitEditor();
        CatalogStore.Save(_catalog);
        if (preset.HandlerId == "use_selected_item"
            && ShowDevTools
            && FocusGameForItemTest
            && !FocusGameWindow())
        {
            AddLog("Could not foreground Dark Souls III; the input will remain queued until the game is active.");
        }
        var result = await _manual.FireAsync(preset.Id);
        AddLog($"{preset.Name}: {result.Status} — {result.Message}");
    }

    private static bool FocusGameWindow()
    {
        foreach (var process in Process.GetProcessesByName("DarkSoulsIII"))
        {
            using (process)
            {
                process.Refresh();
                var window = process.MainWindowHandle;
                if (window == IntPtr.Zero)
                {
                    continue;
                }

                if (IsIconic(window))
                {
                    _ = ShowWindowAsync(window, 9); // SW_RESTORE
                }

                return SetForegroundWindow(window);
            }
        }

        return false;
    }

    [RelayCommand]
    private void Save()
    {
        CommitEditor();
        CatalogStore.Save(_catalog);
        RebuildPresets();
        RebuildEvents();
        AddLog("saved presets");
        _twitch.RequestSync();
    }

    private bool CanTest() => _bridge.State.CanWait && SelectedPreset is not null;

    private async Task StartAsync()
    {
        try
        {
            await _bridge.StartAsync();
            Post(() =>
            {
                AddLog($"Listening on 127.0.0.1:{_bridge.Port}");
                ApplyState();
            });
        }
        catch (Exception exception)
        {
            Post(() => AddLog(exception.Message));
        }
    }

    private void ApplyState()
    {
        var state = _bridge.State;
        IsSafe = state.IsSafe;
        var status = EffectsPaused ? $"Effects paused · {Describe(state)}" : Describe(state);
        if (status != Status)
        {
            Status = status;
        }

        if (status != _loggedStatus)
        {
            _loggedStatus = status;
            AddLog(status);
        }

        TestCommand.NotifyCanExecuteChanged();
    }

    private static bool SameHandlers(IReadOnlyList<HandlerSchema> current, IReadOnlyList<HandlerSchema> incoming)
    {
        if (current.Count != incoming.Count)
        {
            return false;
        }

        for (var index = 0; index < current.Count; index++)
        {
            var left = current[index];
            var right = incoming[index];
            if (left.Id != right.Id || left.Name != right.Name || left.Description != right.Description || left.Arguments.Count != right.Arguments.Count)
            {
                return false;
            }

            for (var argumentIndex = 0; argumentIndex < left.Arguments.Count; argumentIndex++)
            {
                var leftArgument = left.Arguments[argumentIndex];
                var rightArgument = right.Arguments[argumentIndex];
                if (leftArgument.Label != rightArgument.Label || leftArgument.Hint != rightArgument.Hint ||
                    !(leftArgument.Options ?? []).SequenceEqual(rightArgument.Options ?? []))
                {
                    return false;
                }

                if (leftArgument.Name != rightArgument.Name
                    || leftArgument.Type != rightArgument.Type
                    || leftArgument.Default != rightArgument.Default
                    || leftArgument.DefaultText != rightArgument.DefaultText
                    || leftArgument.Min != rightArgument.Min
                    || leftArgument.Max != rightArgument.Max)
                {
                    return false;
                }
            }
        }

        return true;
    }

    private static string Describe(GameState state)
    {
        if (!state.Connected)
        {
            return "Not connected. Start Dark Souls III offline, then attach.";
        }

        if (!state.ExeMatch)
        {
            return "Connected. This Dark Souls III build is not supported.";
        }

        if (!state.SessionKnown)
        {
            return "Connected. Session type is unknown, so effects stay off.";
        }

        if (!state.Offline)
        {
            return "Connected to an online session. Effects stay off.";
        }

        if (!state.InGame)
        {
            return "Offline, waiting for a loaded character.";
        }

        var name = string.IsNullOrWhiteSpace(state.CharacterName) ? "character loaded" : state.CharacterName;
        return $"Offline · {name}";
    }

    private void LoadEditor()
    {
        Arguments.Clear();
        if (SelectedPreset is null)
        {
            EditName = "";
            return;
        }

        EditName = SelectedPreset.Name;
        var handler = _catalog.FindHandler(SelectedPreset.HandlerId);
        if (handler is null)
        {
            return;
        }

        foreach (var argument in handler.Arguments)
        {
            SelectedPreset.Arguments.TryGetValue(argument.Name, out var stored);
            var text = stored.ValueKind switch
            {
                JsonValueKind.String => stored.GetString() ?? "",
                JsonValueKind.Number => stored.ToString(),
                JsonValueKind.Undefined or JsonValueKind.Null => argument.Type == "string" ? argument.DefaultText ?? "" : (argument.Default ?? 0).ToString(),
                _ => stored.ToString()
            };
            Arguments.Add(new ArgumentEditor(argument.Name, argument.Type, text, argument.Label, argument.Hint, argument.Options));
        }
    }

    private void CommitEditor()
    {
        if (SelectedPreset is null)
        {
            return;
        }

        SelectedPreset.Name = EditName;
        var handler = _catalog.FindHandler(SelectedPreset.HandlerId);
        foreach (var argument in Arguments)
        {
            if (argument.Type == "string")
            {
                SelectedPreset.Arguments[argument.Name] = JsonSerializer.SerializeToElement(argument.Value);
                continue;
            }

            if (!int.TryParse(argument.Value, out var number))
            {
                number = handler?.Arguments.FirstOrDefault(item => item.Name == argument.Name)?.Default ?? 0;
            }

            var schema = handler?.Arguments.FirstOrDefault(item => item.Name == argument.Name);
            if (schema?.Min is int min)
            {
                number = Math.Max(min, number);
            }

            if (schema?.Max is int max)
            {
                number = Math.Min(max, number);
            }

            SelectedPreset.Arguments[argument.Name] = JsonSerializer.SerializeToElement(number);
        }
    }

    private void AddLog(string message)
    {
        WriteLogFile(message);
        LogLines.Insert(0, $"{DateTime.Now:HH:mm:ss}  {message}");
        while (LogLines.Count > 200)
        {
            LogLines.RemoveAt(LogLines.Count - 1);
        }
    }

    private static void WriteLogFile(string message)
    {
        try
        {
            var path = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                "Ds3CrowdControl",
                "manager.log");
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.AppendAllText(path, $"{DateTime.Now:yyyy-MM-dd HH:mm:ss}  {message}{Environment.NewLine}");
        }
        catch (IOException)
        {
        }
    }

    private void Post(Action action) => _ui.Post(_ => action(), null);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool SetForegroundWindow(IntPtr window);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool ShowWindowAsync(IntPtr window, int command);

    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool IsIconic(IntPtr window);
}
