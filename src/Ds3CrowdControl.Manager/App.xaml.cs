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
        DispatcherUnhandledException += (_, args) =>
        {
            File.AppendAllText(log, args.Exception + Environment.NewLine);
            args.Handled = true;
        };
        base.OnStartup(e);
        try
        {
            var window = new MainWindow();
            window.Show();
        }
        catch (Exception exception)
        {
            File.WriteAllText(log, exception.ToString());
            Shutdown(1);
        }
    }
}
