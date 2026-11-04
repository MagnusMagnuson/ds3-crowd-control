namespace Ds3CrowdControl.Core;

public sealed record TriggerRequest(string PresetId, string Source);

public interface ITriggerSource
{
    string Name { get; }

    void Start(EffectQueue queue);

    void Stop();
}

public sealed class ManualTriggerSource : ITriggerSource
{
    public string Name => "manual";

    public EffectQueue? Queue { get; private set; }

    public void Start(EffectQueue queue) => Queue = queue;

    public void Stop() => Queue = null;

    public Task<EffectResult> FireAsync(string presetId, CancellationToken cancellationToken = default)
    {
        if (Queue is null)
        {
            return Task.FromResult(new EffectResult("", EffectStatus.Unavailable, "the manual trigger is not attached to a queue"));
        }

        return Queue.EnqueueAsync(presetId, Name, cancellationToken);
    }
}
