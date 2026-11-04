using System.Collections.ObjectModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text.Json;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Ds3CrowdControl.Core;

namespace Ds3CrowdControl.Manager;

public partial class ArgumentEditor : ObservableObject
{
    public ArgumentEditor(string name, string type, string value)
    {
        Name = name;
        Type = type;
        _value = value;
    }

    public string Name { get; }
    public string Type { get; }

    [ObservableProperty]
    private string _value;
}

public partial class MainViewModel : ObservableObject
{
    private readonly SynchronizationContext _ui;
    private readonly Catalog _catalog;
    private readonly ManagerSettings _settings;
    private readonly GameBridgeServer _bridge;
    private readonly EffectQueue _queue;
    private readonly ManualTriggerSource _manual;

    public MainViewModel()
    {
        _ui = SynchronizationContext.Current ?? new SynchronizationContext();
        _catalog = CatalogStore.LoadOrCreate();
        _settings = ManagerSettingsStore.Load();
        _focusGameForItemTest = _settings.FocusGameForItemTest;
        _bridge = new GameBridgeServer(SessionSecret.Create());
        _queue = new EffectQueue(_catalog, _bridge);
        _manual = new ManualTriggerSource();
        _manual.Start(_queue);
        Presets = new ObservableCollection<EffectPreset>(_catalog.Presets);
        _bridge.Logged += message => Post(() => AddLog(message));
        _bridge.StateChanged += () => Post(ApplyState);
        _bridge.HandlersReceived += handlers => Post(() =>
        {
            var handlersChanged = !SameHandlers(_catalog.Handlers, handlers);
            if (handlersChanged)
            {
                _catalog.ReplaceHandlers(handlers);
            }

            var added = new[]
            {
                _catalog.EnsurePreset("Kill player", "kill_player"),
                _catalog.EnsurePreset("Fake death", "fake_death"),
                _catalog.EnsurePreset("Fake death with YOU DIED", "fake_death_staged"),
                _catalog.EnsurePreset("Show YOU DIED", "show_you_died"),
                _catalog.EnsurePreset("Set health to 50%", "set_health_percent"),
                _catalog.EnsurePreset("Set health to random %", "set_health_random_percent"),
                _catalog.EnsurePreset("Watch item button", "watch_item_button")
            }.Where(preset => preset is not null).Cast<EffectPreset>().ToList();
            if (!handlersChanged && added.Count == 0)
            {
                return;
            }

            CatalogStore.Save(_catalog);
            foreach (var preset in added)
            {
                Presets.Add(preset);
            }

            LoadEditor();
        });
        _ = StartAsync();
    }

    public ObservableCollection<EffectPreset> Presets { get; }
    public ObservableCollection<string> LogLines { get; } = [];
    public ObservableCollection<ArgumentEditor> Arguments { get; } = [];

    [ObservableProperty]
    private EffectPreset? _selectedPreset;

    [ObservableProperty]
    private string _status = "Starting the local bridge";

    [ObservableProperty]
    private string _editName = "";

    [ObservableProperty]
    private bool _editEnabled = true;

    [ObservableProperty]
    private int _editCooldown;

    [ObservableProperty]
    private bool _isSafe;

    [ObservableProperty]
    private bool _focusGameForItemTest;

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

    partial void OnEditEnabledChanged(bool value) => TestCommand.NotifyCanExecuteChanged();

    partial void OnFocusGameForItemTestChanged(bool value)
    {
        _settings.FocusGameForItemTest = value;
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

    [RelayCommand(CanExecute = nameof(CanTest))]
    private async Task TestAsync()
    {
        if (SelectedPreset is null)
        {
            return;
        }

        CommitEditor();
        if (SelectedPreset.HandlerId == "use_selected_item"
            && FocusGameForItemTest
            && !FocusGameWindow())
        {
            AddLog("Could not foreground Dark Souls III; the input will remain queued until the game is active.");
        }
        var result = await _manual.FireAsync(SelectedPreset.Id);
        AddLog($"{SelectedPreset.Name}: {result.Status} — {result.Message}");
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
        var selected = SelectedPreset;
        Presets.Clear();
        foreach (var preset in _catalog.Presets)
        {
            Presets.Add(preset);
        }
        SelectedPreset = selected is null ? null : Presets.FirstOrDefault(preset => preset.Id == selected.Id);
        AddLog("saved presets");
    }

    private bool CanTest() => IsSafe && SelectedPreset is not null && EditEnabled;

    private async Task StartAsync()
    {
        try
        {
            await _bridge.StartAsync();
            Post(() => AddLog($"Listening on 127.0.0.1:{_bridge.Port}"));
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
        var status = Describe(state);
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
            EditEnabled = false;
            EditCooldown = 0;
            return;
        }

        EditName = SelectedPreset.Name;
        EditEnabled = SelectedPreset.Enabled;
        EditCooldown = SelectedPreset.CooldownSeconds;
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
            Arguments.Add(new ArgumentEditor(argument.Name, argument.Type, text));
        }
    }

    private void CommitEditor()
    {
        if (SelectedPreset is null)
        {
            return;
        }

        SelectedPreset.Name = EditName;
        SelectedPreset.Enabled = EditEnabled;
        SelectedPreset.CooldownSeconds = Math.Max(0, EditCooldown);
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
        try
        {
            var path = Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
                "Ds3CrowdControl",
                "manager.log");
            Directory.CreateDirectory(Path.GetDirectoryName(path)!);
            File.AppendAllText(path, $"{DateTime.Now:HH:mm:ss}  {message}{Environment.NewLine}");
        }
        catch (IOException)
        {
        }

        LogLines.Insert(0, $"{DateTime.Now:HH:mm:ss}  {message}");
        while (LogLines.Count > 200)
        {
            LogLines.RemoveAt(LogLines.Count - 1);
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
