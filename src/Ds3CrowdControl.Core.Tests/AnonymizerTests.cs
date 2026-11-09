using Ds3CrowdControl.Core;
using Xunit;

namespace Ds3CrowdControl.Core.Tests;

public class AnonymizerTests
{
    private readonly Anonymizer _anonymizer = new(@"C:\Users\Jane Doe", "Jane Doe", "JANES-LAPTOP");

    [Fact]
    public void The_user_folder_becomes_userprofile_in_every_spelling()
    {
        Assert.Equal(@"%USERPROFILE%\Desktop\a.zip", _anonymizer.Clean(@"C:\Users\Jane Doe\Desktop\a.zip"));
        Assert.Equal(@"%USERPROFILE%\AppData", _anonymizer.Clean(@"c:\users\jane doe\AppData"));
        Assert.Equal(@"{""p"":""%USERPROFILE%\\x""}", _anonymizer.Clean(@"{""p"":""C:\\Users\\Jane Doe\\x""}"));
        Assert.Equal("%USERPROFILE%/x", _anonymizer.Clean("C:/Users/Jane Doe/x"));
    }

    [Fact]
    public void The_user_and_pc_names_are_replaced_wherever_they_stand_alone()
    {
        Assert.Equal(@"D:\%USERNAME%\OneDrive\game", _anonymizer.Clean(@"D:\Jane Doe\OneDrive\game"));
        Assert.Equal("signed in as %USERNAME% on %COMPUTERNAME%", _anonymizer.Clean("signed in as jane doe on janes-laptop"));
    }

    [Fact]
    public void A_short_name_inside_another_word_is_left_alone()
    {
        var anonymizer = new Anonymizer(@"C:\Users\ann", "ann", "PC");
        Assert.Equal("announce items, %USERNAME%, %COMPUTERNAME%", anonymizer.Clean("announce items, ANN, pc"));
        Assert.Equal("PCs and APC stay", anonymizer.Clean("PCs and APC stay"));
    }

    [Fact]
    public void Missing_names_change_nothing()
    {
        Assert.Equal(@"C:\Users\x", new Anonymizer(null, "", null).Clean(@"C:\Users\x"));
    }
}
