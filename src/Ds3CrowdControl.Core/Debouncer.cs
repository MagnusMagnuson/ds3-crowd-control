namespace Ds3CrowdControl.Core;

/// <summary>Runs an action once, after requests have stopped for the given delay.</summary>
public sealed class Debouncer(TimeSpan delay, Func<Task> action)
{
    private readonly object _gate = new();
    private CancellationTokenSource? _waiting;

    public void Request()
    {
        var waiting = new CancellationTokenSource();
        lock (_gate)
        {
            _waiting?.Cancel();
            _waiting = waiting;
        }

        _ = RunAsync(waiting.Token);
    }

    public void Cancel()
    {
        lock (_gate)
        {
            _waiting?.Cancel();
            _waiting = null;
        }
    }

    private async Task RunAsync(CancellationToken cancellationToken)
    {
        try
        {
            await Task.Delay(delay, cancellationToken).ConfigureAwait(false);
            await action().ConfigureAwait(false);
        }
        catch (OperationCanceledException)
        {
        }
    }
}
