using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;

// The job handle is never inherited. Closing it, including on supervisor death,
// terminates every descendant, even if the original process has already exited.
public sealed class OwnedProcess : IDisposable
{
    private IntPtr job;
    private IntPtr process;
    public uint Id { get; private set; }

    public OwnedProcess(string executable, string arguments)
    {
        job = CreateJobObject(IntPtr.Zero, null);
        if (job == IntPtr.Zero) throw new Win32Exception();
        try
        {
            var limits = new ExtendedLimits();
            limits.Basic.LimitFlags = 0x2000; // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            if (!SetInformationJobObject(job, 9, ref limits, (uint)Marshal.SizeOf(limits)))
                throw new Win32Exception();

            var startup = new StartupInfo();
            startup.Size = Marshal.SizeOf(startup);
            ProcessInfo child;
            // Suspend before the first instruction: no child can escape assignment.
            if (!CreateProcess(executable, new StringBuilder("\"" + executable + "\" " + arguments),
                IntPtr.Zero, IntPtr.Zero, false, 0x08000004, IntPtr.Zero, null, ref startup, out child))
                throw new Win32Exception(); // CREATE_NO_WINDOW | CREATE_SUSPENDED
            process = child.Process;
            Id = child.Id;
            try
            {
                if (!AssignProcessToJobObject(job, process)) throw new Win32Exception();
                if (ResumeThread(child.Thread) == uint.MaxValue) throw new Win32Exception();
            }
            catch
            {
                TerminateProcess(process, 1); // The exact child we created, still suspended on assignment failure.
                throw;
            }
            finally { CloseHandle(child.Thread); }
        }
        catch { Dispose(); throw; }
    }

    public bool Wait(int milliseconds)
    {
        uint result = WaitForSingleObject(process, (uint)milliseconds);
        if (result == uint.MaxValue) throw new Win32Exception();
        return result == 0;
    }

    public void Dispose()
    {
        if (job != IntPtr.Zero) { CloseHandle(job); job = IntPtr.Zero; }
        if (process != IntPtr.Zero) { CloseHandle(process); process = IntPtr.Zero; }
    }

    [StructLayout(LayoutKind.Sequential)] struct BasicLimits
    {
        public long ProcessTime, JobTime;
        public uint LimitFlags;
        public UIntPtr MinimumWorkingSet, MaximumWorkingSet;
        public uint ActiveProcessLimit;
        public UIntPtr Affinity;
        public uint PriorityClass, SchedulingClass;
    }
    [StructLayout(LayoutKind.Sequential)] struct IoCounters
    {
        public ulong ReadOperations, WriteOperations, OtherOperations, ReadBytes, WriteBytes, OtherBytes;
    }
    [StructLayout(LayoutKind.Sequential)] struct ExtendedLimits
    {
        public BasicLimits Basic;
        public IoCounters Io;
        public UIntPtr ProcessMemory, JobMemory, PeakProcessMemory, PeakJobMemory;
    }
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)] struct StartupInfo
    {
        public int Size;
        public string Reserved, Desktop, Title;
        public uint X, Y, XSize, YSize, XChars, YChars, Fill, Flags;
        public ushort ShowWindow, ReservedSize;
        public IntPtr ReservedBytes, Input, Output, Error;
    }
    [StructLayout(LayoutKind.Sequential)] struct ProcessInfo
    {
        public IntPtr Process, Thread;
        public uint Id, ThreadId;
    }
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr CreateJobObject(IntPtr attributes, string name);
    [DllImport("kernel32.dll", SetLastError = true)]
    static extern bool SetInformationJobObject(IntPtr job, int kind, ref ExtendedLimits limits, uint size);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CreateProcess(string executable, StringBuilder command, IntPtr processAttributes,
        IntPtr threadAttributes, bool inherit, uint flags, IntPtr environment, string directory,
        ref StartupInfo startup, out ProcessInfo child);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool AssignProcessToJobObject(IntPtr job, IntPtr process);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll", SetLastError = true)] static extern uint WaitForSingleObject(IntPtr handle, uint milliseconds);
    [DllImport("kernel32.dll", SetLastError = true)] static extern bool TerminateProcess(IntPtr process, uint code);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr handle);
}
