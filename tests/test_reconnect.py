"""A previously paired device must reconnect without starting pairing again."""
from ipaddress import IPv4Address
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, Mock, patch
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'Support'))
from bridge import Remote


class ReconnectTests(unittest.IsolatedAsyncioTestCase):
    async def test_legacy_credentials_reconnect_and_persist_selection(self):
        with tempfile.TemporaryDirectory() as data, patch.object(sys, 'argv', ['bridge', data]):
            remote = Remote()
            remote.credentials = {'tv-id': {'Companion': 'test-credential'}}
            config = SimpleNamespace(name='Living Room', address=IPv4Address('192.0.2.11'), set_credentials=Mock())
            original_handle = remote.handle
            async def handle(request):
                if request['action'] == 'scan':
                    remote.devices = {'tv-id': config}
                    return {'devices': []}
                return await original_handle(request)
            remote.handle = handle
            atv = SimpleNamespace(close=Mock())
            with patch('bridge.pyatv.connect', new=AsyncMock(return_value=atv)), patch('bridge.pyatv.pair', new=AsyncMock()) as pair, patch.object(remote, 'watch_metadata', new=AsyncMock()):
                result = await remote.handle({'action': 'reconnect'})
                self.assertTrue(result['paired'])
                self.assertEqual(result['device']['id'], 'tv-id')
                pair.assert_not_awaited()
                config.set_credentials.assert_called_once()
                self.assertEqual(remote.preferences['last_device'], 'tv-id')
                await remote.close_connection()
                self.assertIsNone(remote.metadata_task)

    async def test_missing_saved_device_does_not_start_pairing(self):
        with tempfile.TemporaryDirectory() as data, patch.object(sys, 'argv', ['bridge', data]):
            remote = Remote()
            self.assertEqual(await remote.handle({'action': 'reconnect'}), {'paired': False})


if __name__ == '__main__':
    unittest.main()
