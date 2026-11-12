// FCP's video rooms (its `docs/SOFTPHONE_VIDEO.md` §2.6): who is in the room
// a call joined, who speaks and who shares, a host's controls over it, and
// the meetings sheet — joining by number or link, and a link to send for
// each of the user's own meetings.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';
import 'contacts.dart';

/// The room a call is in: who speaks or shares, the people, and (for a
/// host) the room's controls.
class RoomPanel extends StatelessWidget {
  const RoomPanel({super.key, required this.model, required this.call});
  final PhoneModel model;
  final PhoneCall call;

  RoomInfo get room => call.room!;

  Future<void> _control(
    RoomControl control, {
    String? member,
    bool on = false,
  }) => model.run(
    () => model.api.roomControl(call.id, control, member: member, on: on),
  );

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final theme = Theme.of(context);
    final speaking = room.speaking;
    return Card(
      key: const Key('roomPanel'),
      margin: const EdgeInsets.only(top: 12),
      child: Padding(
        padding: const EdgeInsets.symmetric(vertical: 8),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (room.presenter case final presenter?)
              ListTile(
                key: const Key('roomPresenter'),
                dense: true,
                leading: const Icon(Icons.present_to_all),
                title: Text(s.roomPresenting(presenter)),
                trailing: room.host
                    ? TextButton(
                        key: const Key('roomStopShare'),
                        onPressed: () => _control(RoomControl.stopShare),
                        child: Text(s.roomStopShare),
                      )
                    : null,
              ),
            if (room.floorRefused case final why?)
              ListTile(
                key: const Key('roomShareStopped'),
                dense: true,
                leading: Icon(
                  Icons.cancel_presentation,
                  color: theme.colorScheme.error,
                ),
                title: Text(s.roomShareStopped(shareReason(s, why))),
              ),
            if (call.isConnected && model.snapshot.video.screens.isNotEmpty)
              _ShareRow(model: model, call: call),
            if (speaking != null && call.isConnected)
              ListTile(
                key: const Key('roomSpeaking'),
                dense: true,
                leading: const Icon(Icons.record_voice_over),
                title: Text(s.roomSpeaking(speaking.name)),
              ),
            if (room.host) _HostBar(onControl: _control),
            ExpansionTile(
              key: const Key('roomPeople'),
              initiallyExpanded: room.members.length <= 8,
              leading: const Icon(Icons.groups),
              title: Text(s.roomPeople(room.members.length)),
              children: [
                for (final m in room.members)
                  _MemberTile(
                    member: m,
                    speaker: m.id == room.speaker,
                    host: room.host,
                    onControl: _control,
                  ),
              ],
            ),
          ],
        ),
      ),
    );
  }
}

/// Why our share stopped, as the room names it, in words.
String shareReason(Strings s, String why) => switch (why) {
  'host' => s.roomShareWhyHost,
  'held' => s.roomShareWhyHeld,
  'disabled' => s.roomShareWhyDisabled,
  'participant_disabled' => s.roomShareWhyNotYou,
  'idle' => s.roomShareWhyIdle,
  'failed' || 'no_source' => s.roomShareWhyFailed,
  'budget' => s.roomShareWhyBusy,
  _ => why,
};

/// Our own share: on or off, and which screen where there is a choice.
class _ShareRow extends StatelessWidget {
  const _ShareRow({required this.model, required this.call});
  final PhoneModel model;
  final PhoneCall call;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final video = model.snapshot.video;
    final sharing = call.room!.sharing;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      child: Wrap(
        spacing: 8,
        runSpacing: 8,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          FilledButton.tonalIcon(
            key: const Key('roomShare'),
            icon: Icon(
              sharing ? Icons.cancel_presentation : Icons.present_to_all,
            ),
            label: Text(sharing ? s.roomStopSharing : s.roomShare),
            onPressed: () => model.shareScreen(call.id, !sharing),
          ),
          if (video.screens.length > 1)
            DropdownButton<String>(
              key: const Key('roomScreen'),
              hint: Text(s.roomChooseScreen),
              value: video.screens.any((d) => d.id == video.screen)
                  ? video.screen
                  : null,
              items: [
                for (final d in video.screens)
                  DropdownMenuItem(value: d.id, child: Text(d.name)),
              ],
              onChanged: (id) => model.run(() => model.api.chooseScreen(id)),
            ),
          if (sharing)
            Text(
              s.roomSharing,
              key: const Key('roomSharing'),
              style: Theme.of(context).textTheme.bodySmall,
            ),
        ],
      ),
    );
  }
}

typedef _OnControl = Future<void> Function(
  RoomControl control, {
  String? member,
  bool on,
});

/// A host's controls over the whole room.
class _HostBar extends StatelessWidget {
  const _HostBar({required this.onControl});
  final _OnControl onControl;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
      child: Wrap(
        key: const Key('roomHostBar'),
        spacing: 8,
        runSpacing: 8,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: [
          Chip(
            avatar: const Icon(Icons.shield_outlined, size: 16),
            label: Text(s.roomYouHost),
          ),
          OutlinedButton.icon(
            key: const Key('roomGrid'),
            onPressed: () => onControl(RoomControl.layout, on: true),
            icon: const Icon(Icons.grid_view, size: 18),
            label: Text(s.roomGrid),
          ),
          OutlinedButton.icon(
            key: const Key('roomSpeakerLayout'),
            onPressed: () => onControl(RoomControl.layout),
            icon: const Icon(Icons.view_sidebar_outlined, size: 18),
            label: Text(s.roomSpeakerLayout),
          ),
          TextButton(
            key: const Key('roomUnpin'),
            onPressed: () => onControl(RoomControl.pin),
            child: Text(s.roomUnpin),
          ),
          TextButton(
            key: const Key('roomStopSpotlight'),
            onPressed: () => onControl(RoomControl.spotlight),
            child: Text(s.roomStopSpotlight),
          ),
        ],
      ),
    );
  }
}

/// One person in the room, with a host's controls over them.
class _MemberTile extends StatelessWidget {
  const _MemberTile({
    required this.member,
    required this.speaker,
    required this.host,
    required this.onControl,
  });
  final RoomMember member;
  final bool speaker;
  final bool host;
  final _OnControl onControl;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final theme = Theme.of(context);
    final m = member;
    final about = [
      if (m.host) s.roomHost,
      if (m.audio == 'muted') s.roomMuted,
      if (m.audio == 'held') s.roomHeld,
      if (m.camera != null && !m.cameraOn) s.roomCameraOff,
    ];
    return ListTile(
      key: Key('member-${m.id}'),
      dense: true,
      leading: CircleAvatar(
        radius: 16,
        backgroundColor: speaker
            ? theme.colorScheme.primary
            : theme.colorScheme.surfaceContainerHighest,
        foregroundColor: speaker
            ? theme.colorScheme.onPrimary
            : theme.colorScheme.onSurface,
        child: Icon(
          m.speaking ? Icons.graphic_eq : Icons.person,
          size: 18,
          key: m.speaking ? Key('speaking-${m.id}') : null,
        ),
      ),
      title: Text(m.name.isEmpty ? m.id : m.name),
      subtitle: about.isEmpty ? null : Text(about.join(' · ')),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (m.presenting)
            const Padding(
              padding: EdgeInsets.only(right: 4),
              child: Icon(Icons.present_to_all, size: 18),
            ),
          Icon(m.muted ? Icons.mic_off : Icons.mic, size: 18),
          if (m.camera != null)
            Padding(
              padding: const EdgeInsets.only(left: 4),
              child: Icon(
                m.cameraOn ? Icons.videocam : Icons.videocam_off,
                size: 18,
              ),
            ),
          if (host)
            Builder(
              builder: (anchor) => IconButton(
                key: Key('memberMenu-${m.id}'),
                tooltip: s.roomMemberActions(m.name),
                icon: const Icon(Icons.more_vert),
                onPressed: () => showMoreMenu(anchor, [
                  MoreAction(
                    key: Key('memberMute-${m.id}'),
                    icon: m.audio == 'muted' ? Icons.mic : Icons.mic_off,
                    label: m.audio == 'muted'
                        ? s.roomUnmuteMember
                        : s.roomMuteMember,
                    onSelected: () => onControl(
                      RoomControl.mute,
                      member: m.id,
                      on: m.audio != 'muted',
                    ),
                  ),
                  if (m.camera != null)
                    MoreAction(
                      key: Key('memberCamera-${m.id}'),
                      icon: m.cameraOn ? Icons.videocam_off : Icons.videocam,
                      label: m.cameraOn ? s.roomStopCamera : s.roomAllowCamera,
                      onSelected: () => onControl(
                        RoomControl.camera,
                        member: m.id,
                        on: !m.cameraOn,
                      ),
                    ),
                  MoreAction(
                    key: Key('memberPin-${m.id}'),
                    icon: Icons.push_pin_outlined,
                    label: s.roomPin,
                    onSelected: () => onControl(RoomControl.pin, member: m.id),
                  ),
                  MoreAction(
                    key: Key('memberSpotlight-${m.id}'),
                    icon: Icons.highlight,
                    label: s.roomSpotlight,
                    onSelected: () =>
                        onControl(RoomControl.spotlight, member: m.id),
                  ),
                  MoreAction(
                    key: Key('memberRemove-${m.id}'),
                    icon: Icons.person_remove_outlined,
                    label: s.roomRemove,
                    onSelected: () =>
                        onControl(RoomControl.remove, member: m.id),
                  ),
                ]),
              ),
            ),
        ],
      ),
    );
  }
}

/// Joining a room, by its number or a link, and the user's own meetings
/// with a link to send for each.
class MeetingsSheet extends StatefulWidget {
  const MeetingsSheet({super.key, required this.model, this.link});
  final PhoneModel model;

  /// A link the system handed the app, ready to join.
  final String? link;

  /// Show the sheet over `context`.
  static Future<void> show(
    BuildContext context,
    PhoneModel model, {
    String? link,
  }) => showModalBottomSheet<void>(
    context: context,
    isScrollControlled: true,
    builder: (context) => SafeArea(
      child: Padding(
        padding: EdgeInsets.only(
          bottom: MediaQuery.of(context).viewInsets.bottom,
        ),
        child: ListenableBuilder(
          listenable: model,
          builder: (context, _) => MeetingsSheet(model: model, link: link),
        ),
      ),
    ),
  );

  @override
  State<MeetingsSheet> createState() => _MeetingsSheetState();
}

class _MeetingsSheetState extends State<MeetingsSheet> {
  late final _target = TextEditingController(text: widget.link ?? '');

  PhoneModel get model => widget.model;

  @override
  void initState() {
    super.initState();
    _target.addListener(() => setState(() {}));
  }

  @override
  void dispose() {
    _target.dispose();
    super.dispose();
  }

  Future<void> _join(String target, {required bool video}) async {
    final navigator = Navigator.of(context);
    await model.joinRoom(target.trim(), video: video);
    if (model.notice == null && navigator.canPop()) navigator.pop();
  }

  Future<void> _copy(Meeting m, {required bool host}) async {
    final messenger = ScaffoldMessenger.maybeOf(context);
    final s = Strings.of(context);
    final link = await model.meetingLink(m.id, host: host);
    if (link == null) return;
    await Clipboard.setData(ClipboardData(text: link));
    messenger?.showSnackBar(SnackBar(content: Text(s.linkCopied)));
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final theme = Theme.of(context);
    final snap = model.snapshot;
    final video = snap.video.available;
    final ready = _target.text.trim().isNotEmpty;
    return Padding(
      key: const Key('meetingsSheet'),
      padding: const EdgeInsets.all(16),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(s.meetingsButton, style: theme.textTheme.titleLarge),
          const SizedBox(height: 12),
          TextField(
            key: const Key('joinTarget'),
            controller: _target,
            autofocus: widget.link == null,
            decoration: InputDecoration(
              labelText: s.joinRoomLabel,
              border: const OutlineInputBorder(),
            ),
            onSubmitted: ready ? (t) => _join(t, video: false) : null,
          ),
          const SizedBox(height: 8),
          Wrap(
            alignment: WrapAlignment.end,
            spacing: 8,
            children: [
              OutlinedButton.icon(
                key: const Key('join'),
                onPressed: ready
                    ? () => _join(_target.text, video: false)
                    : null,
                icon: const Icon(Icons.call),
                label: Text(s.joinButton),
              ),
              if (video)
                FilledButton.icon(
                  key: const Key('joinVideo'),
                  onPressed: ready
                      ? () => _join(_target.text, video: true)
                      : null,
                  icon: const Icon(Icons.videocam),
                  label: Text(s.joinWithVideoButton),
                ),
            ],
          ),
          const SizedBox(height: 16),
          Text(s.yourMeetings, style: theme.textTheme.titleSmall),
          if (snap.meetings.isEmpty)
            Padding(
              padding: const EdgeInsets.symmetric(vertical: 12),
              child: Text(s.noMeetings, key: const Key('noMeetings')),
            ),
          for (final m in snap.meetings)
            ListTile(
              key: Key('meeting-${m.id}'),
              contentPadding: EdgeInsets.zero,
              leading: const Icon(Icons.meeting_room_outlined),
              title: Text(m.name),
              subtitle: Text(m.roomNumber),
              trailing: Wrap(
                spacing: 4,
                children: [
                  IconButton(
                    key: Key('meetingJoin-${m.id}'),
                    tooltip: video ? s.joinWithVideoButton : s.joinButton,
                    icon: Icon(video ? Icons.videocam : Icons.call),
                    onPressed: () => _join(m.roomNumber, video: video),
                  ),
                  Builder(
                    builder: (anchor) => IconButton(
                      key: Key('meetingLinks-${m.id}'),
                      tooltip: s.copyLinkButton,
                      icon: const Icon(Icons.link),
                      onPressed: () => showMoreMenu(anchor, [
                        MoreAction(
                          key: Key('meetingLink-${m.id}'),
                          icon: Icons.link,
                          label: s.copyLinkButton,
                          onSelected: () => _copy(m, host: false),
                        ),
                        MoreAction(
                          key: Key('meetingHostLink-${m.id}'),
                          icon: Icons.shield_outlined,
                          label: s.copyHostLinkButton,
                          onSelected: () => _copy(m, host: true),
                        ),
                      ]),
                    ),
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}
