using System.Text;
using System.Text.RegularExpressions;

namespace Ds3CrowdControl.Core;

/// <summary>
/// Takes the Windows user folder, user name and PC name out of text that leaves the PC.
/// Many people use their real name for these, so a support zip must not contain them.
/// </summary>
public sealed class Anonymizer
{
    public const string UserFolderStandIn = "%USERPROFILE%";
    public const string UserNameStandIn = "%USERNAME%";
    public const string ComputerNameStandIn = "%COMPUTERNAME%";

    private readonly string? _userFolder;
    private readonly Regex? _userName;
    private readonly Regex? _computerName;

    public Anonymizer(string? userFolder, string? userName, string? computerName)
    {
        _userFolder = string.IsNullOrWhiteSpace(userFolder) ? null : userFolder.TrimEnd('\\', '/');
        _userName = WholeName(userName);
        _computerName = WholeName(computerName);
    }

    public static Anonymizer ForThisPc() => new(
        Environment.GetFolderPath(Environment.SpecialFolder.UserProfile),
        Environment.UserName,
        Environment.MachineName);

    /// <summary>
    /// The folder goes first, so its paths read %USERPROFILE%\... The names are then replaced
    /// wherever they stand on their own, also inside other paths such as a OneDrive folder.
    /// Matching whole names only keeps a short name like "ann" from changing "announce".
    /// </summary>
    public string Clean(string text)
    {
        if (_userFolder is not null)
        {
            text = text
                .Replace(_userFolder.Replace(@"\", @"\\"), UserFolderStandIn, StringComparison.OrdinalIgnoreCase)
                .Replace(_userFolder, UserFolderStandIn, StringComparison.OrdinalIgnoreCase)
                .Replace(_userFolder.Replace('\\', '/'), UserFolderStandIn, StringComparison.OrdinalIgnoreCase);
        }

        if (_userName is not null)
        {
            text = _userName.Replace(text, UserNameStandIn);
        }

        if (_computerName is not null)
        {
            text = _computerName.Replace(text, ComputerNameStandIn);
        }

        return text;
    }

    public byte[] Clean(byte[] bytes) => Encoding.UTF8.GetBytes(Clean(Encoding.UTF8.GetString(bytes)));

    private static Regex? WholeName(string? name) =>
        string.IsNullOrWhiteSpace(name)
            ? null
            : new Regex($@"(?<![\p{{L}}\p{{N}}_]){Regex.Escape(name)}(?![\p{{L}}\p{{N}}_])", RegexOptions.IgnoreCase | RegexOptions.CultureInvariant);
}
