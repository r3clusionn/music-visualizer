"""Runs mviz in a real window and checks it: the window appears and draws, and how much CPU it
uses. Windows only (it finds the window with the Win32 API).

    python scripts/live_check.py [FILE] [--shot window.png]

With FILE the file is shown without sound (--silent); then mviz runs once more in loopback mode
(what the computer is playing) to check that capture starts. Needs Pillow and psutil.
"""
import ctypes
import ctypes.wintypes as wt
import os
import subprocess
import sys
import time

import psutil
from PIL import ImageGrab

user32 = ctypes.windll.user32
EXE = os.path.join(os.path.dirname(__file__), "..", "target", "release", "mviz.exe")


def window_of(pid):
    found = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(hwnd, _):
        p = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(p))
        if p.value == pid and user32.IsWindowVisible(hwnd):
            found.append(hwnd)
        return True

    user32.EnumWindows(cb, 0)
    return found[0] if found else None


def client_rect(hwnd):
    r = wt.RECT()
    user32.GetClientRect(hwnd, ctypes.byref(r))
    pt = wt.POINT(0, 0)
    user32.ClientToScreen(hwnd, ctypes.byref(pt))
    return (pt.x, pt.y, pt.x + r.right, pt.y + r.bottom)


def run(args, label, shot=None):
    p = subprocess.Popen([EXE] + args, stderr=subprocess.PIPE)
    try:
        hwnd = None
        for _ in range(50):
            time.sleep(0.1)
            hwnd = window_of(p.pid)
            if hwnd or p.poll() is not None:
                break
        if not hwnd:
            print(f"{label}: no window (exit {p.poll()}): {p.stderr.read().decode() if p.poll() is not None else ''}")
            return False
        user32.SetForegroundWindow(hwnd)
        time.sleep(2.0)
        proc = psutil.Process(p.pid)
        c0 = proc.cpu_times()
        t0 = time.perf_counter()
        time.sleep(5.0)
        c1 = proc.cpu_times()
        cpu = (c1.user + c1.system - c0.user - c0.system) / (time.perf_counter() - t0) * 100
        img = ImageGrab.grab(bbox=client_rect(hwnd))
        colours = len(set(img.get_flattened_data()))
        if shot:
            img.save(shot)
        print(f"{label}: window {img.size[0]}x{img.size[1]}, {colours} colours on screen, CPU {cpu:.1f}% of one core, memory {proc.memory_info().rss / 2**20:.0f} MB")
        return p.poll() is None
    finally:
        p.kill()


args = [a for a in sys.argv[1:] if not a.startswith("--")]
shot = sys.argv[sys.argv.index("--shot") + 1] if "--shot" in sys.argv else None
ok = True
if args:
    ok &= run([args[0], "--silent", "-e", "spectrogram,bars"], "file, silent", shot)
ok &= run(["-e", "bars"], "loopback")
sys.exit(0 if ok else 1)
