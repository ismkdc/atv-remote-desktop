"""Line-delimited JSON transport. stdout is reserved for IPC."""
import asyncio
from contextlib import suppress
import json
import os
import sys
from pathlib import Path
import pyatv
import ifaddr
from ipaddress import ip_address
from pyatv.const import Protocol, FeatureName, FeatureState, KeyboardFocusState, DeviceState
from pyatv.exceptions import NotSupportedError, InvalidStateError

class Remote:
    def __init__(self):
        self.devices = {}
        self.atv = None
        self.pairing = None
        self.selected = None
        self.playing = None
        self.metadata_task = None
        self.path = Path(sys.argv[1]) / 'credentials.json'
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.credentials = json.loads(self.path.read_text()) if self.path.exists() else {}
        self.preferences_path = self.path.with_name('preferences.json')
        self.preferences = json.loads(self.preferences_path.read_text()) if self.preferences_path.exists() else {}

    def available(self, feature):
        return self.atv.features.get_feature(feature).state == FeatureState.Available

    async def context(self):
        result = {'connected': self.atv is not None, 'skip_backward': False,
                  'skip_forward': False, 'text_input': False, 'media': None}
        if not self.atv:
            return result
        # Unknown/unsupported capabilities must never become visible controls.
        try:
            result['text_input'] = (
                self.available(FeatureName.TextSet)
                and self.available(FeatureName.TextFocusState)
                and self.atv.keyboard.text_focus_state == KeyboardFocusState.Focused
            )
        except (NotSupportedError, InvalidStateError):
            pass
        playing = self.playing
        active = playing is None or playing.device_state in (
            DeviceState.Playing, DeviceState.Paused, DeviceState.Seeking,
        )
        result['skip_backward'] = active and self.available(FeatureName.SkipBackward)
        result['skip_forward'] = active and self.available(FeatureName.SkipForward)
        if playing is not None and active:
            media = {key: getattr(playing, key) for key in ('title', 'artist', 'album', 'position', 'total_time')}
            if any(media[key] for key in ('title', 'artist', 'album')):
                result['media'] = {**media, 'state': playing.device_state.name}
        return result

    async def watch_metadata(self, atv):
        """Collect slow metadata off the serialized command/IPC path."""
        while self.atv is atv:
            try:
                if any(self.available(feature) for feature in (
                    FeatureName.Title, FeatureName.Artist, FeatureName.Album,
                    FeatureName.Position, FeatureName.TotalTime,
                )):
                    playing = await asyncio.wait_for(atv.metadata.playing(), timeout=1.5)
                    if self.atv is atv:
                        self.playing = playing
                else:
                    self.playing = None
            except Exception:
                self.playing = None
            await asyncio.sleep(1.5)

    async def close_connection(self):
        if self.metadata_task:
            self.metadata_task.cancel()
            with suppress(asyncio.CancelledError):
                await self.metadata_task
            self.metadata_task = None
        self.playing = None
        if self.atv:
            self.atv.close()
            self.atv = None

    async def handle(self, request):
        action = request['action']
        loop = asyncio.get_running_loop()
        if action == 'reconnect':
            identifier = self.preferences.get('last_device') or next(reversed(self.credentials), None)
            if not identifier or not self.credentials.get(identifier):
                return {'paired': False}
            await self.handle({'action': 'scan'})
            config = self.devices.get(identifier)
            if not config:
                return {'paired': False, 'reason': 'not_found'}
            result = await self.handle({'action': 'connect', 'id': identifier})
            return {**result, 'device': {'id': identifier, 'name': config.name, 'address': str(config.address)}}
        if action == 'context':
            return await self.context()
        if action == 'health':
            return {'ok': True, 'discovery': 'Bonjour' if sys.platform == 'darwin' else 'mDNS'}
        if action == 'scan':
            if sys.platform == 'darwin':
                from native_bonjour import scan
                configs = await scan()
            else:
                configs = await pyatv.scan(loop, timeout=6)
            local_addresses = {
                str(ip.ip) for adapter in ifaddr.get_adapters()
                for ip in adapter.ips if isinstance(ip.ip, str)
            }
            self.devices = {
                str(c.identifier or c.address): c for c in configs
                if c.ready and not ip_address(str(c.address)).is_loopback
                and str(c.address) not in local_addresses
            }
            return {'devices': [{'id': k, 'name': c.name, 'address': str(c.address)} for k, c in self.devices.items()]}
        if action in ('connect', 'pair_begin'):
            identifier = request['id']
            config = self.devices.get(identifier)
            if not config:
                raise ValueError('Scan for devices again.')
            self.selected = identifier
            saved = self.credentials.get(identifier, {})
            for name, value in saved.items():
                config.set_credentials(Protocol[name], value)
            if action == 'connect':
                if not saved:
                    return {'paired': False}
                await self.close_connection()
                self.atv = await pyatv.connect(config, loop)
                self.metadata_task = asyncio.create_task(self.watch_metadata(self.atv))
                self.preferences['last_device'] = identifier
                self.preferences_path.write_text(json.dumps(self.preferences))
                return {'paired': True}
            if self.pairing:
                await self.pairing.close()
            protocol = Protocol.Companion if config.get_service(Protocol.Companion) else Protocol.AirPlay
            self.protocol = protocol
            self.pairing = await pyatv.pair(config, protocol, loop)
            await self.pairing.begin()
            return {'ok': True}
        if action == 'pair_finish':
            if not self.pairing:
                raise ValueError('Start pairing first.')
            self.pairing.pin(request['pin'])
            await self.pairing.finish()
            if not self.pairing.has_paired:
                raise ValueError('The PIN could not be verified.')
            self.credentials.setdefault(self.selected, {})[self.protocol.name] = str(self.pairing.service.credentials)
            # Restrict credential file to the current user, including during creation.
            fd = os.open(self.path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
            with os.fdopen(fd, 'w') as output:
                json.dump(self.credentials, output)
            os.chmod(self.path, 0o600)
            await self.pairing.close()
            self.pairing = None
            return {'ok': True}
        if action == 'command':
            if not self.atv:
                raise ValueError('No Apple TV is connected.')
            command = request['command']
            if command == 'power':
                from pyatv.const import PowerState
                if self.atv.power.power_state == PowerState.On:
                    await self.atv.power.turn_off()
                else:
                    await self.atv.power.turn_on()
            elif command == 'mute':
                volume = self.atv.audio.volume
                await self.atv.audio.set_volume(0 if volume > 0 else getattr(self, 'last_volume', 25))
                if volume > 0:
                    self.last_volume = volume
            elif command == 'text':
                if not (await self.context())['text_input']:
                    raise ValueError('Select a text field on your Apple TV first.')
                await self.atv.keyboard.text_set(request.get('text', ''))
            elif command in {'skip_backward', 'skip_forward'}:
                if not (await self.context())[command]:
                    raise ValueError('Seeking is not available for the current media.')
                await getattr(self.atv.remote_control, command)(10.0)
            elif command in {'up','down','left','right','select','menu','home','play_pause'}:
                await getattr(self.atv.remote_control, command)()
            else:
                raise ValueError('This feature is not supported by the connected device.')
            return {'ok': True}
        raise ValueError('Unknown action')

async def main():
    remote = Remote()
    try:
        while True:
            line = await asyncio.to_thread(sys.stdin.readline)
            if not line:
                break
            try:
                result = await remote.handle(json.loads(line))
            except Exception as error:
                result = {'error': str(error) or type(error).__name__}
            print(json.dumps(result), flush=True)
    finally:
        if remote.pairing:
            await remote.pairing.close()
        await remote.close_connection()

if __name__ == '__main__':
    asyncio.run(main())
