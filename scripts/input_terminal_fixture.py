#!/usr/bin/env python3
"""Receive only one fixed synthetic cursor-test string; never execute input.

Usage: python3 -I scripts/input_terminal_fixture.py /unused/report.json
Start only in a dedicated test terminal. Unexpected input is not saved.
"""
import json
import os
import pathlib
import select
import sys
import termios
import time
import tty

EXPECTED = '终端输入验证🙂'.encode()
output = pathlib.Path(sys.argv[1])
if output.exists():
    raise SystemExit('Report already exists; choose an unused path')
print('VoicePort 独立终端输入验证：只接收固定测试文字，不执行命令。', flush=True)
settings = termios.tcgetattr(sys.stdin.fileno())
received = bytearray()
reason = 'timeout'
try:
    tty.setraw(sys.stdin.fileno())
    deadline = time.monotonic() + 180
    while time.monotonic() < deadline:
        ready, _, _ = select.select([sys.stdin], [], [], min(1, deadline - time.monotonic()))
        if not ready:
            continue
        data = os.read(sys.stdin.fileno(), 4096)
        if not data:
            reason = 'closed'
            break
        received.extend(data)
        if not EXPECTED.startswith(received):
            reason = 'unexpected input; content discarded'
            break
        if received == EXPECTED:
            reason = 'matched fixed UTF-8 text exactly; no Return'
            break
finally:
    termios.tcsetattr(sys.stdin.fileno(), termios.TCSADRAIN, settings)
report = {'passed': received == EXPECTED, 'received_bytes': len(received), 'expected_bytes': len(EXPECTED), 'reason': reason}
with output.open('x') as stream:
    json.dump(report, stream, ensure_ascii=False)
    stream.write('\n')
print('\n' + json.dumps(report, ensure_ascii=False), flush=True)
