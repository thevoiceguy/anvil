// The microphone and the speaker, and how loud each is: in Settings and in
// a call's Audio sheet (`docs/APP.md` §4). A call that is up moves to the
// devices chosen and takes the volumes at once. Settings also tests them:
// the microphone recorded and heard back, a sound on each speaker.

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

class AudioControls extends StatelessWidget {
  const AudioControls({super.key, required this.snap, required this.model});
  final PhoneSnapshot snap;
  final PhoneModel model;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return Column(
      mainAxisSize: MainAxisSize.min,
      children: [
        DevicePicker(
          key: const Key('microphone'),
          label: s.microphone,
          icon: Icons.mic,
          devices: snap.inputs,
          chosen: snap.input,
          onChosen: (id) =>
              model.run(() => model.api.chooseAudio(input: true, device: id)),
        ),
        if (snap.microphoneVolume case final percent?)
          VolumeSlider(
            key: const Key('microphoneVolume'),
            label: s.microphoneVolume,
            icon: Icons.mic_none,
            percent: percent,
            onChanged: (p) =>
                model.run(() => model.api.setAudioVolume(microphone: p)),
          ),
        DevicePicker(
          key: const Key('speaker'),
          label: s.speaker,
          icon: Icons.volume_up,
          devices: snap.outputs,
          chosen: snap.output,
          onChosen: (id) =>
              model.run(() => model.api.chooseAudio(input: false, device: id)),
        ),
        if (snap.speakerVolume case final percent?)
          VolumeSlider(
            key: const Key('speakerVolume'),
            label: s.speakerVolume,
            icon: Icons.volume_down,
            percent: percent,
            onChanged: (p) =>
                model.run(() => model.api.setAudioVolume(speaker: p)),
          ),
      ],
    );
  }
}

/// A microphone or a speaker to choose, or the system's default.
class DevicePicker extends StatelessWidget {
  const DevicePicker({
    super.key,
    required this.label,
    required this.icon,
    required this.devices,
    required this.chosen,
    required this.onChosen,
  });

  final String label;
  final IconData icon;
  final List<AudioDevice> devices;
  final String? chosen;
  final ValueChanged<String?> onChosen;

  // DropdownMenu's entries need a value; the system's default is ''.
  static const _system = '';

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final known = devices.any((d) => d.id == chosen);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      child: DropdownMenu<String>(
        expandedInsets: EdgeInsets.zero,
        leadingIcon: Icon(icon),
        label: Text(label),
        initialSelection: known ? chosen : _system,
        dropdownMenuEntries: [
          DropdownMenuEntry(value: _system, label: s.systemDefault),
          for (final d in devices)
            DropdownMenuEntry(value: d.id, label: d.name),
        ],
        onSelected: (id) {
          if (id == null) return;
          onChosen(id == _system ? null : id);
        },
      ),
    );
  }
}

/// A volume from 0 to 200 percent, 100 the sound as it is: moved freely,
/// sent when let go.
class VolumeSlider extends StatefulWidget {
  const VolumeSlider({
    super.key,
    required this.label,
    required this.icon,
    required this.percent,
    required this.onChanged,
  });

  final String label;
  final IconData icon;
  final int percent;
  final ValueChanged<int> onChanged;

  static const max = 200;

  @override
  State<VolumeSlider> createState() => _VolumeSliderState();
}

class _VolumeSliderState extends State<VolumeSlider> {
  double? _dragging;

  @override
  Widget build(BuildContext context) {
    final value = (_dragging ?? widget.percent.toDouble()).clamp(
      0.0,
      VolumeSlider.max.toDouble(),
    );
    return ListTile(
      leading: Icon(widget.icon),
      title: Text(widget.label),
      subtitle: Slider(
        value: value,
        max: VolumeSlider.max.toDouble(),
        divisions: VolumeSlider.max ~/ 5,
        label: '${value.round()}%',
        onChanged: (v) => setState(() => _dragging = v),
        onChangeEnd: (v) {
          setState(() => _dragging = null);
          widget.onChanged(v.round());
        },
      ),
      trailing: Text('${value.round()}%'),
    );
  }
}

/// Tests of the microphone and the speakers, with the volumes set: the
/// microphone's level while it records, then what it heard played back.
/// Not while a call is up: the phone refuses.
class AudioTests extends StatelessWidget {
  const AudioTests({super.key, required this.snap, required this.model});
  final PhoneSnapshot snap;
  final PhoneModel model;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final test = snap.audioTest;
    final busy = test != null || snap.calls.isNotEmpty;
    final status = switch (test) {
      AudioTest.recording => s.testRecording,
      AudioTest.playingBack => s.testPlayingBack,
      AudioTest.speaker => s.testSpeakerPlaying,
      AudioTest.ring => s.testRingPlaying,
      null => s.testMicrophoneNote,
    };
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        mainAxisSize: MainAxisSize.min,
        children: [
          Wrap(
            spacing: 8,
            runSpacing: 8,
            children: [
              OutlinedButton.icon(
                key: const Key('testMicrophone'),
                icon: const Icon(Icons.mic),
                label: Text(s.testMicrophone),
                onPressed: busy
                    ? null
                    : () => model.run(() => model.api.testMicrophone()),
              ),
              OutlinedButton.icon(
                key: const Key('testSpeaker'),
                icon: const Icon(Icons.volume_up),
                label: Text(s.testSpeaker),
                onPressed: busy
                    ? null
                    : () => model.run(() => model.api.testSpeaker(ring: false)),
              ),
              OutlinedButton.icon(
                key: const Key('testRinging'),
                icon: const Icon(Icons.notifications_active),
                label: Text(s.testRinging),
                onPressed: busy
                    ? null
                    : () => model.run(() => model.api.testSpeaker(ring: true)),
              ),
              if (test != null)
                TextButton.icon(
                  key: const Key('stopTest'),
                  icon: const Icon(Icons.stop),
                  label: Text(s.testStop),
                  onPressed: () => model.run(() => model.api.stopAudioTest()),
                ),
            ],
          ),
          const SizedBox(height: 8),
          Text(status, style: Theme.of(context).textTheme.bodySmall),
          if (test == AudioTest.recording) ...[
            const SizedBox(height: 8),
            LinearProgressIndicator(
              key: const Key('microphoneLevel'),
              value: snap.audioTestLevel / 100,
              minHeight: 8,
            ),
          ],
        ],
      ),
    );
  }
}
