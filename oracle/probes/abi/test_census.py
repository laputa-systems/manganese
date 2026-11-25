import tempfile
import unittest
from pathlib import Path

import census


class CensusTest(unittest.TestCase):
    def test_proc_fd_reads_are_not_paths_and_relative_opens_are(self):
        with tempfile.TemporaryDirectory(dir=census.ROOT / '.work/tmp') as temporary:
            root = Path(temporary)
            trace = root / 'session'
            trace.mkdir()
            (trace / 'status').write_text('0\n')
            (trace / 'trace.1').write_text(
                'execve("/opt/browser/chrome", ["/opt/browser/chrome"], 0x123) = 0\n'
                'openat(AT_FDCWD, "/proc/123/task/456/status", O_RDONLY) = 3</proc/123/task/456/status>\n'
                'read(3</proc/123/task/456/status>, "Name:\\\\tchrome", 100) = 12\n'
                'openat(4</sys/devices/system/cpu>, "cpu7/online", O_RDONLY) = 5</sys/devices/system/cpu/cpu7/online>\n'
                'madvise(0x123, 4096, MADV_DONTNEED) = 0\n'
                'socket(AF_UNIX, SOCK_STREAM|SOCK_CLOEXEC, 0) = 6\n'
                'io_uring_setup(4, 0x123) = -1 ENOSYS (Function not implemented)\n'
            )
            result = census.census(root)
            paths = {item for category, item in result if category == 'path'}
            self.assertEqual(paths, {'/proc/<pid>/task/<tid>/status', '/sys/devices/system/cpu/cpu<number>/online'})
            self.assertEqual(result[('madvise', 'MADV_DONTNEED')], {'browser'})
            self.assertEqual(result[('socket', 'AF_UNIX,SOCK_STREAM|SOCK_CLOEXEC,0')], {'browser'})
            self.assertEqual(result[('error', 'io_uring_setup:ENOSYS')], {'browser'})


if __name__ == '__main__':
    unittest.main()
