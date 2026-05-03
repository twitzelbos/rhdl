// Headless ContrAlto trace dumper.
//
// Boots a disk image and runs N cycles of the AltoSystem.SingleStep()
// loop, dumping per-cycle CPU state as TSV to stdout for cross-checking
// against rhdl-alto's chip output.
//
// Usage:
//   dotnet run --project crates/rhdl-alto/tools/contralto-trace --
//     --disk <path-to-.dsk>
//     --rom-dir <path-with-Conf-and-AltoII-subdirs>
//     --cycles <N>
//
// Output (TSV, header on first line):
//   cycle  task  mpc   t      l      ir     mar    aluc0  r0    r1    ...

using System;
using System.IO;
using System.Linq;
using Contralto;
using Contralto.Display;

namespace ContraltoTrace
{
    static class Program
    {
        static int Main(string[] args)
        {
            string diskPath = "";
            string romDir = "";
            int cycles = 2000;
            for (int i = 0; i < args.Length; i++)
            {
                if (args[i] == "--disk" && i + 1 < args.Length) diskPath = args[++i];
                else if (args[i] == "--rom-dir" && i + 1 < args.Length) romDir = args[++i];
                else if (args[i] == "--cycles" && i + 1 < args.Length) cycles = int.Parse(args[++i]);
            }
            if (string.IsNullOrEmpty(diskPath) || string.IsNullOrEmpty(romDir))
            {
                Console.Error.WriteLine("usage: contralto-trace --disk <path> --rom-dir <path> [--cycles N]");
                return 2;
            }
            if (!File.Exists(diskPath))
            {
                Console.Error.WriteLine($"disk image not found: {diskPath}");
                return 2;
            }

            // ContraltoLib looks up its ROMs via Configuration.GetAltoIIRomPath
            // (combines RomPath + "AltoII/<file>") AND Configuration.GetRomPath
            // (combines RomPath + "<file>") for top-level files like
            // ACSOURCE.NEW.  Easiest: just point RomPath at ContrAlto's
            // bundled ROM/ directory (which has both the AltoII subdir +
            // the top-level auxiliary ROMs).  Ignore --rom-dir entirely
            // since the bytes are byte-identical anyway.
            _ = romDir;
            StartupOptions.RomPath = Path.GetFullPath(
                "crates/rhdl-alto/assets/contralto/Contralto2/ContraltoLib/ROM");

            var config = new Configuration();
            config.SystemType = SystemType.TwoKRom;
            config.Drive0Image = diskPath;
            config.AlternateBootType = AlternateBootType.Disk;
            config.BootAddress = 0;  // sector 0
            config.HostPacketInterfaceType = PacketInterfaceType.None;

            var system = new AltoSystem(config);
            system.PressBootKeys(AlternateBootType.Disk);

            var cpu = system.CPU;
            // Header.
            Console.Write("cycle\ttask\tmpc\tt\tl\tir\taluc0");
            for (int r = 0; r < 32; r++) Console.Write($"\tr{r}");
            Console.WriteLine();

            for (int c = 0; c < cycles; c++)
            {
                system.SingleStep();
                var task = cpu.CurrentTask;
                int taskIndex = task != null ? (int)task.TaskType : -1;
                ushort mpc = task != null ? task.MPC : (ushort)0xffff;
                Console.Write($"{c}\t{taskIndex}\t0x{mpc:x3}\t0x{cpu.T:x4}\t0x{cpu.L:x4}\t0x{cpu.IR:x4}\t{cpu.ALUC0}");
                for (int r = 0; r < 32; r++) Console.Write($"\t0x{cpu.R[r]:x4}");
                Console.WriteLine();
            }

            return 0;
        }
    }
}
