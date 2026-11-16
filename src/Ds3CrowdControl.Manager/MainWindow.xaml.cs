using System.Windows;

namespace Ds3CrowdControl.Manager;

public partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();
        var viewModel = new MainViewModel();
        DataContext = viewModel;
        Closing += (_, _) =>
        {
            viewModel.PauseTwitchRewards();
            viewModel.SetDiscordOffline();
        };
        SaveDiscordTokenButton.Click += (_, _) =>
        {
            viewModel.SaveDiscordToken(DiscordTokenBox.Password);
            DiscordTokenBox.Clear();
        };
    }
}
