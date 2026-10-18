"""Run inside the release image with its Chromium seccomp profile (amd64/arm64)."""
import ctypes
import errno
import os
import platform
import subprocess
import sys
import threading
import unittest


@unittest.skipUnless(sys.platform == 'linux' and platform.machine() in ('x86_64', 'aarch64'),
                     'Linux amd64/arm64 syscall smoke test')
class SeccompRuntimeTests(unittest.TestCase):
    def setUp(self):
        self.libc = ctypes.CDLL(None, use_errno=True)
        self.libc.syscall.restype = ctypes.c_long

    def test_runc_can_close_unused_descriptors(self):
        self.assertEqual(self.libc.syscall(436, 1000000, 1000000, 0), 0,
                         'close_range blocked: errno=' + str(ctypes.get_errno()))

    def test_runc_can_open_procfs_with_openat2(self):
        class OpenHow(ctypes.Structure):
            _fields_ = [('flags', ctypes.c_uint64), ('mode', ctypes.c_uint64),
                        ('resolve', ctypes.c_uint64)]
        how = OpenHow(os.O_PATH | os.O_DIRECTORY, 0, 0)
        descriptor = self.libc.syscall(437, -100, b'/proc/self/fd', ctypes.byref(how), ctypes.sizeof(how))
        self.assertGreaterEqual(descriptor, 0, 'openat2 blocked: errno=' + str(ctypes.get_errno()))
        os.close(descriptor)

    def test_clone3_returns_enosys_for_glibc_fallback(self):
        self.assertEqual(self.libc.syscall(435, None, 0), -1)
        self.assertEqual(ctypes.get_errno(), errno.ENOSYS)

    def test_threads_and_child_processes_work(self):
        completed = []
        thread = threading.Thread(target=lambda: completed.append(True))
        thread.start()
        thread.join(timeout=5)
        self.assertEqual(completed, [True])
        result = subprocess.run([sys.executable, '-c', 'print("child-ok")'],
                                capture_output=True, text=True, timeout=5, check=True)
        self.assertEqual(result.stdout.strip(), 'child-ok')


if __name__ == '__main__':
    unittest.main()
