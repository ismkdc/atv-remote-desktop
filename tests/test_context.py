"""Contextual controls must follow live capabilities, playback and keyboard focus."""
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch
from pyatv.const import DeviceState, FeatureName, FeatureState, KeyboardFocusState
from pyatv.interface import Playing
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'Support'))
from bridge import Remote


class ContextTests(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.data = tempfile.TemporaryDirectory()
        with patch.object(sys, 'argv', ['bridge', self.data.name]):
            self.remote = Remote()
        self.states = {}
        self.keyboard = SimpleNamespace(text_focus_state=KeyboardFocusState.Unfocused, text_set=AsyncMock())
        self.metadata = SimpleNamespace(playing=AsyncMock(return_value=Playing(device_state=DeviceState.Playing, title='Test movie')))
        self.remote.playing = self.metadata.playing.return_value
        self.control = SimpleNamespace(skip_forward=AsyncMock())
        self.remote.atv = SimpleNamespace(
            features=SimpleNamespace(get_feature=lambda feature: SimpleNamespace(state=self.states.get(feature, FeatureState.Unsupported))),
            metadata=self.metadata, keyboard=self.keyboard, remote_control=self.control)

    def tearDown(self):
        self.data.cleanup()

    async def test_playback_and_feature_changes_remove_controls(self):
        self.states.update({FeatureName.Title: FeatureState.Available, FeatureName.SkipForward: FeatureState.Available})
        context = await self.remote.context()
        self.assertTrue(context['skip_forward'])
        self.assertFalse(context['skip_backward'])
        self.assertEqual(context['media']['title'], 'Test movie')
        self.remote.playing = Playing(device_state=DeviceState.Idle, title='Stale movie')
        context = await self.remote.context()
        self.assertFalse(context['skip_forward'])
        self.assertIsNone(context['media'])
        self.states[FeatureName.SkipForward] = FeatureState.Unknown
        self.remote.playing = Playing(device_state=DeviceState.Playing)
        self.assertFalse((await self.remote.context())['skip_forward'])

    async def test_keyboard_requires_both_focus_and_text_support(self):
        self.states.update({FeatureName.TextFocusState: FeatureState.Available, FeatureName.TextSet: FeatureState.Available})
        self.assertFalse((await self.remote.context())['text_input'])
        self.keyboard.text_focus_state = KeyboardFocusState.Focused
        self.assertTrue((await self.remote.context())['text_input'])
        self.states[FeatureName.TextSet] = FeatureState.Unavailable
        self.assertFalse((await self.remote.context())['text_input'])

    async def test_companion_skip_flags_work_without_metadata(self):
        self.remote.playing = None
        self.states[FeatureName.SkipForward] = FeatureState.Available
        self.assertTrue((await self.remote.context())['skip_forward'])
        self.metadata.playing.assert_not_awaited()
        self.states[FeatureName.SkipForward] = FeatureState.Unavailable
        with self.assertRaisesRegex(ValueError, 'Seeking is not available'):
            await self.remote.handle({'action': 'command', 'command': 'skip_forward'})
        self.control.skip_forward.assert_not_awaited()
        self.states[FeatureName.SkipForward] = FeatureState.Available
        await self.remote.handle({'action': 'command', 'command': 'skip_forward'})
        self.control.skip_forward.assert_awaited_once_with(10.0)

    async def test_disconnected_context_has_no_controls(self):
        self.remote.atv = None
        context = await self.remote.context()
        self.assertFalse(context['connected'])
        self.assertFalse(any(context[key] for key in ('skip_forward', 'skip_backward', 'text_input', 'media')))


if __name__ == '__main__':
    unittest.main()
