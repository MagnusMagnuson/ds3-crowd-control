using System.IO;
using System.Windows;

namespace Ds3CrowdControl.Manager;

public partial class App : Application
{
    protected override void OnStartup(StartupEventArgs e)
    {
        var log = Path.Combine(
            Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData),
            "Ds3CrowdControl",
            "manager.log");
        Directory.CreateDirectory(Path.GetDirectoryName(log)!);
        void Record(string kind, object error)
        {
            try
            {
                File.AppendAllText(log, $"{DateTime.Now:yyyy-MM-dd HH:mm:ss}  {kind}: {error}{Environment.NewLine}");
            }
            catch (IOException)
            {
            }
        }

        DispatcherUnhandledException += (_, args) =>
        {
            Record("unexpected error", args.Exception);
            args.Handled = true;
        };
        // Background work, such as Twitch, fails on other threads where nothing else would see it.
        AppDomain.CurrentDomain.UnhandledException += (_, args) => Record("crash", args.ExceptionObject);
        System.Threading.Tasks.TaskScheduler.UnobservedTaskException += (_, args) =>
        {
            Record("background error", args.Exception);
            args.SetObserved();
        };
        base.OnStartup(e);
        try
        {
            var window = new MainWindow();
            window.Show();
        }
        catch (Exception exception)
        {
            Record("could not start", exception);
            Shutdown(1);
        }
    }
}
