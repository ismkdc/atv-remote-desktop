"""Regression coverage for device selection and IPC-safe discovery failures."""
import asyncio
from ipaddress import IPv4Address
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'Support'))
from bridge import Remote


class DiscoveryTests(unittest.IsolatedAsyncioTestCase):
    async def test_scan_excludes_loopback_and_this_machine(self):
        def config(address, identifier, ready=True):
            return SimpleNamespace(address=IPv4Address(address), identifier=identifier,
                                   name=identifier, ready=ready)
        configs = [config('127.0.0.1', 'loopback'), config('192.0.2.10', 'this Mac'),
                   config('192.0.2.11', 'Living Room'), config('192.0.2.12', 'not ready', False)]
        adapter = SimpleNamespace(ips=[SimpleNamespace(ip='192.0.2.10'), SimpleNamespace(ip=('::1', 0, 0))])
        with tempfile.TemporaryDirectory() as data, patch.object(sys, 'argv', ['bridge', data]):
            remote = Remote()
            with patch('bridge.sys.platform', 'linux'), patch('bridge.pyatv.scan', new=AsyncMock(return_value=configs)), patch('bridge.ifaddr.get_adapters', return_value=[adapter]):
                result = await remote.handle({'action': 'scan'})
            self.assertEqual(result['devices'], [{'id': 'Living Room', 'name': 'Living Room', 'address': '192.0.2.11'}])
            self.assertEqual(list(remote.devices), ['Living Room'])

    async def test_scan_propagates_network_errors_instead_of_empty_success(self):
        with tempfile.TemporaryDirectory() as data, patch.object(sys, 'argv', ['bridge', data]):
            remote = Remote()
            with patch('bridge.sys.platform', 'linux'), patch('bridge.pyatv.scan', new=AsyncMock(side_effect=PermissionError('Network blocked'))):
                with self.assertRaisesRegex(PermissionError, 'Network blocked'):
                    await remote.handle({'action': 'scan'})


if __name__ == '__main__':
    unittest.main()
