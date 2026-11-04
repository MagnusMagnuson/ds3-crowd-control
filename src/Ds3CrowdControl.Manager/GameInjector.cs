using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;

namespace Ds3CrowdControl.Manager;

public static class GameInjector
{
    public static (bool Ok, string Message) Inject(string dllPath)
    {
        if (!Environment.Is64BitProcess)
        {
            return (false, "the manager has to run as a 64-bit process");
        }

        if (!File.Exists(dllPath))
        {
            return (false, $"ds3mod.dll was not found at {dllPath}");
        }

        var game = FindDarkSouls();
        if (game is null)
        {
            return (false, "Dark Souls III is not running");
        }

        var pathBytes = Encoding.Unicode.GetBytes(dllPath + "\0");
        var process = Native.OpenProcess(
            Native.ProcessCreateThread | Native.ProcessQueryInformation | Native.ProcessVmOperation | Native.ProcessVmWrite | Native.ProcessVmRead,
            false,
            game.Id);
        if (process == 0)
        {
            return (false, "Dark Souls III is running, but the mod could not attach to it");
        }

        nint remote = 0;
        nint thread = 0;
        try
        {
            remote = Native.VirtualAllocEx(process, 0, (uint)pathBytes.Length, Native.MemCommit | Native.MemReserve, Native.PageReadWrite);
            if (remote == 0)
            {
                return (false, "the game did not accept the mod path");
            }

            if (!Native.WriteProcessMemory(process, remote, pathBytes, pathBytes.Length, out _))
            {
                return (false, "the mod path could not be written into the game");
            }

            var kernel = Native.GetModuleHandle("kernel32.dll");
            var loadLibrary = Native.GetProcAddress(kernel, "LoadLibraryW");
            if (loadLibrary == 0)
            {
                return (false, "LoadLibraryW was not found");
            }

            thread = Native.CreateRemoteThread(process, 0, 0, loadLibrary, remote, 0, out _);
            if (thread == 0)
            {
                return (false, "the game did not start the mod");
            }

            Native.WaitForSingleObject(thread, 10_000);
            Native.GetExitCodeThread(thread, out var module);
            if (module == 0)
            {
                return (false, "Dark Souls III did not load ds3mod.dll");
            }

            return (true, "ds3mod.dll is loaded into DarkSoulsIII.exe");
        }
        finally
        {
            if (thread != 0)
            {
                Native.CloseHandle(thread);
            }

            if (remote != 0)
            {
                Native.VirtualFreeEx(process, remote, 0, Native.MemRelease);
            }

            Native.CloseHandle(process);
            game.Dispose();
        }
    }

    private static Process? FindDarkSouls()
    {
        foreach (var process in Process.GetProcessesByName("DarkSoulsIII"))
        {
            try
            {
                var moduleName = process.MainModule?.ModuleName;
                if (moduleName is not null && moduleName.Equals("DarkSoulsIII.exe", StringComparison.OrdinalIgnoreCase))
                {
                    return process;
                }
            }
            catch (Win32Exception)
            {
            }

            process.Dispose();
        }

        return null;
    }

    private static class Native
    {
        public const uint ProcessCreateThread = 0x0002;
        public const uint ProcessQueryInformation = 0x0400;
        public const uint ProcessVmOperation = 0x0008;
        public const uint ProcessVmWrite = 0x0020;
        public const uint ProcessVmRead = 0x0010;
        public const uint MemCommit = 0x1000;
        public const uint MemReserve = 0x2000;
        public const uint MemRelease = 0x8000;
        public const uint PageReadWrite = 0x04;

        [DllImport("kernel32.dll", SetLastError = true)]
        public static extern nint OpenProcess(uint access, bool inherit, int processId);

        [DllImport("kernel32.dll", SetLastError = true)]
        public static extern nint VirtualAllocEx(nint process, nint address, uint size, uint allocationType, uint protect);

        [DllImport("kernel32.dll", SetLastError = true)]
        public static extern bool VirtualFreeEx(nint process, nint address, uint size, uint freeType);

        [DllImport("kernel32.dll", SetLastError = true)]
        public static extern bool WriteProcessMemory(nint process, nint address, byte[] buffer, int size, out nint written);

        [DllImport("kernel32.dll", SetLastError = true)]
        public static extern nint CreateRemoteThread(nint process, nint attributes, nuint stackSize, nint start, nint parameter, uint flags, out uint threadId);

        [DllImport("kernel32.dll", SetLastError = true)]
        public static extern uint WaitForSingleObject(nint handle, uint milliseconds);

        [DllImport("kernel32.dll", SetLastError = true)]
        public static extern bool GetExitCodeThread(nint thread, out uint exitCode);

        [DllImport("kernel32.dll", SetLastError = true)]
        public static extern bool CloseHandle(nint handle);

        [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
        public static extern nint GetModuleHandle(string name);

        [DllImport("kernel32.dll", CharSet = CharSet.Ansi, SetLastError = true)]
        public static extern nint GetProcAddress(nint module, string name);
    }
}
