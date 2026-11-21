// A room's audience (FCP's `docs/WEBINARS.md` §2.5 and §2.9, A7): a
// webinar's phase and the stage, raised hands, questions, and chat in every
// room. What one says goes in the call (`PhoneApi.roomAudience`); a host's
// room controls go to FCP (`PhoneApi.roomControl`).

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

/// Say `action` in the room `call` is in.
Future<void> _say(
  PhoneModel model,
  PhoneCall call,
  AudienceAction action, {
  String? id,
  String? text,
  bool on = false,
}) => model.run(
  () => model.api.roomAudience(call.id, action, id: id, text: text, on: on),
);

/// A host's control of `call`'s room.
Future<void> _control(
  PhoneModel model,
  PhoneCall call,
  RoomControl control, {
  String? member,
  bool on = false,
}) => model.run(
  () => model.api.roomControl(call.id, control, member: member, on: on),
);

/// A webinar's state: the lobby, practice or live with the audience's
/// count, the stage, and a host's go live and end.
class WebinarBanner extends StatelessWidget {
  const WebinarBanner({super.key, required this.model, required this.call});
  final PhoneModel model;
  final PhoneCall call;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final theme = Theme.of(context);
    final room = call.room!;
    final phase = room.webinar;
    final (icon, title) = switch (phase) {
      _ when room.lobby => (Icons.hourglass_top, s.webinarLobby),
      'practice' => (Icons.mic_external_on, s.webinarPractice),
      'ended' => (Icons.stop_circle_outlined, s.webinarEnded),
      _ => (Icons.sensors, s.webinarLive),
    };
    final audience = room.audience;
    return Column(
      key: const Key('webinarBanner'),
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        ListTile(
          dense: true,
          leading: Icon(
            icon,
            color: phase == 'live' && !room.lobby
                ? theme.colorScheme.error
                : null,
          ),
          title: Text(title, key: const Key('webinarPhase')),
          subtitle: audience == null
              ? null
              : Text(
                  s.webinarWatching(audience),
                  key: const Key('webinarAudience'),
                ),
        ),
        if (room.onStage)
          ListTile(
            key: const Key('webinarOnStage'),
            dense: true,
            leading: Icon(Icons.campaign, color: theme.colorScheme.primary),
            title: Text(s.webinarOnStage),
          )
        else if (room.listening && !room.lobby)
          ListTile(
            key: const Key('webinarListening'),
            dense: true,
            leading: const Icon(Icons.headphones),
            title: Text(s.webinarListening),
          ),
        if (room.host && phase != 'ended')
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
            child: Wrap(
              spacing: 8,
              children: [
                if (phase == 'practice')
                  FilledButton.icon(
                    key: const Key('webinarGoLive'),
                    icon: const Icon(Icons.sensors),
                    label: Text(s.webinarGoLive),
                    onPressed: () => _control(model, call, RoomControl.goLive),
                  ),
                OutlinedButton.icon(
                  key: const Key('webinarEnd'),
                  icon: const Icon(Icons.stop_circle_outlined),
                  label: Text(s.webinarEnd),
                  onPressed: () async {
                    final ok = await showDialog<bool>(
                      context: context,
                      builder: (c) => AlertDialog(
                        content: Text(s.webinarEndConfirm),
                        actions: [
                          TextButton(
                            onPressed: () => Navigator.pop(c, false),
                            child: Text(
                              MaterialLocalizations.of(c).cancelButtonLabel,
                            ),
                          ),
                          FilledButton(
                            key: const Key('webinarEndConfirm'),
                            onPressed: () => Navigator.pop(c, true),
                            child: Text(s.webinarEnd),
                          ),
                        ],
                      ),
                    );
                    if (ok == true) {
                      await _control(model, call, RoomControl.endWebinar);
                    }
                  },
                ),
              ],
            ),
          ),
        if (room.role == 'attendee' &&
            !room.onStage &&
            !room.lobby &&
            phase == 'live')
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 4),
            child: Align(
              alignment: Alignment.centerLeft,
              child: FilledButton.tonalIcon(
                key: const Key('webinarHand'),
                icon: Icon(
                  room.hand ? Icons.back_hand : Icons.back_hand_outlined,
                ),
                label: Text(
                  room.hand ? s.webinarLowerHand : s.webinarRaiseHand,
                ),
                onPressed: () =>
                    _say(model, call, AudienceAction.hand, on: !room.hand),
              ),
            ),
          ),
      ],
    );
  }
}

/// The raised hands, for a host: lower one or all, bring one to the stage.
class HandsPanel extends StatelessWidget {
  const HandsPanel({super.key, required this.model, required this.call});
  final PhoneModel model;
  final PhoneCall call;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final room = call.room!;
    final hands = room.hands;
    return ExpansionTile(
      key: const Key('webinarHands'),
      initiallyExpanded: hands.isNotEmpty,
      leading: const Icon(Icons.back_hand_outlined),
      title: Text(s.webinarHands(hands.length)),
      trailing: hands.isEmpty
          ? null
          : TextButton(
              key: const Key('webinarLowerAll'),
              onPressed: () => _control(model, call, RoomControl.lowerAllHands),
              child: Text(s.webinarLowerAll),
            ),
      children: [
        for (final m in hands)
          ListTile(
            key: Key('webinarHand-${m.id}'),
            dense: true,
            title: Text(m.name),
            trailing: Wrap(
              spacing: 4,
              children: [
                TextButton(
                  key: Key('webinarToStage-${m.id}'),
                  onPressed: () => _control(
                    model,
                    call,
                    RoomControl.stage,
                    member: m.id,
                    on: true,
                  ),
                  child: Text(s.webinarToStage),
                ),
                TextButton(
                  onPressed: () => _control(
                    model,
                    call,
                    RoomControl.lowerHand,
                    member: m.id,
                  ),
                  child: Text(s.webinarLowerTheirs),
                ),
              ],
            ),
          ),
      ],
    );
  }
}

/// The questions we may see: asked, upvoted, and for the stage published,
/// answered and dismissed.
class QuestionsPanel extends StatefulWidget {
  const QuestionsPanel({super.key, required this.model, required this.call});
  final PhoneModel model;
  final PhoneCall call;

  @override
  State<QuestionsPanel> createState() => _QuestionsPanelState();
}

class _QuestionsPanelState extends State<QuestionsPanel> {
  final _ask = TextEditingController();
  bool _anonymous = false;

  @override
  void dispose() {
    _ask.dispose();
    super.dispose();
  }

  Future<void> _answer(BuildContext context, RoomQuestion q) async {
    final s = Strings.of(context);
    final text = TextEditingController();
    final answer = await showDialog<String>(
      context: context,
      builder: (c) => AlertDialog(
        title: Text(q.text),
        content: TextField(
          key: const Key('roomAnswerField'),
          controller: text,
          maxLength: 1000,
          decoration: InputDecoration(hintText: s.roomAnswerHint),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.pop(c),
            child: Text(MaterialLocalizations.of(c).cancelButtonLabel),
          ),
          FilledButton(
            key: const Key('roomAnswerSend'),
            onPressed: () => Navigator.pop(c, text.text),
            child: Text(s.roomSend),
          ),
        ],
      ),
    );
    text.dispose();
    if (answer != null && answer.trim().isNotEmpty) {
      await _say(
        widget.model,
        widget.call,
        AudienceAction.answer,
        id: q.id,
        text: answer,
      );
    }
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final theme = Theme.of(context);
    final room = widget.call.room!;
    final questions = [...room.questions]
      ..sort((a, b) => b.upvotes.compareTo(a.upvotes));
    return ExpansionTile(
      key: const Key('roomQuestions'),
      leading: const Icon(Icons.help_outline),
      title: Text(s.roomQuestions(questions.length)),
      children: [
        for (final q in questions)
          ListTile(
            key: Key('roomQuestion-${q.id}'),
            dense: true,
            title: Text(q.text),
            subtitle: Text(
              [
                q.asker ?? (q.mine ? s.roomYou : '—'),
                if (q.answered) s.roomAnswered,
                if (!q.published) s.roomNotPublished,
                if (q.answer case final a?) '“$a”',
              ].join(' · '),
              style: theme.textTheme.bodySmall,
            ),
            trailing: Wrap(
              crossAxisAlignment: WrapCrossAlignment.center,
              children: [
                if (!q.mine)
                  TextButton.icon(
                    key: Key('roomUpvote-${q.id}'),
                    icon: Icon(
                      q.upvoted ? Icons.thumb_up : Icons.thumb_up_outlined,
                      size: 16,
                    ),
                    label: Text('${q.upvotes}'),
                    onPressed: () => _say(
                      widget.model,
                      widget.call,
                      AudienceAction.upvote,
                      id: q.id,
                      on: !q.upvoted,
                    ),
                  )
                else
                  Text('${q.upvotes}'),
                if (room.moderates)
                  PopupMenuButton<String>(
                    key: Key('roomModerate-${q.id}'),
                    onSelected: (what) => switch (what) {
                      'publish' => _say(
                        widget.model,
                        widget.call,
                        AudienceAction.publish,
                        id: q.id,
                      ),
                      'aloud' => _say(
                        widget.model,
                        widget.call,
                        AudienceAction.answer,
                        id: q.id,
                        on: true,
                      ),
                      'text' => _answer(context, q),
                      _ => _say(
                        widget.model,
                        widget.call,
                        AudienceAction.dismiss,
                        id: q.id,
                      ),
                    },
                    itemBuilder: (_) => [
                      if (!q.published)
                        PopupMenuItem(
                          value: 'publish',
                          child: Text(s.roomPublish),
                        ),
                      PopupMenuItem(
                        value: 'aloud',
                        child: Text(s.roomAnswerAloud),
                      ),
                      PopupMenuItem(
                        value: 'text',
                        child: Text(s.roomAnswerText),
                      ),
                      PopupMenuItem(
                        value: 'dismiss',
                        child: Text(s.roomDismiss),
                      ),
                    ],
                  ),
              ],
            ),
          ),
        Padding(
          padding: const EdgeInsets.fromLTRB(16, 4, 8, 8),
          child: Row(
            children: [
              Expanded(
                child: TextField(
                  key: const Key('roomAskField'),
                  controller: _ask,
                  maxLength: 500,
                  decoration: InputDecoration(
                    hintText: s.roomAsk,
                    counterText: '',
                  ),
                ),
              ),
              Tooltip(
                message: s.roomAskAnonymously,
                child: Checkbox(
                  key: const Key('roomAskAnonymous'),
                  value: _anonymous,
                  onChanged: (v) => setState(() => _anonymous = v ?? false),
                ),
              ),
              IconButton(
                key: const Key('roomAskSend'),
                icon: const Icon(Icons.send),
                tooltip: s.roomSend,
                onPressed: () async {
                  final text = _ask.text.trim();
                  if (text.isEmpty) return;
                  _ask.clear();
                  await _say(
                    widget.model,
                    widget.call,
                    AudienceAction.ask,
                    text: text,
                    on: _anonymous,
                  );
                },
              ),
            ],
          ),
        ),
      ],
    );
  }
}

/// Chat in the room: the messages we may see, ours to post (to the room or
/// its hosts), and for the stage deleting and muting.
class ChatPanel extends StatefulWidget {
  const ChatPanel({super.key, required this.model, required this.call});
  final PhoneModel model;
  final PhoneCall call;

  @override
  State<ChatPanel> createState() => _ChatPanelState();
}

class _ChatPanelState extends State<ChatPanel> {
  final _text = TextEditingController();
  bool _toHosts = false;

  @override
  void dispose() {
    _text.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final theme = Theme.of(context);
    final room = widget.call.room!;
    final chat = room.chat;
    return ExpansionTile(
      key: const Key('roomChat'),
      leading: const Icon(Icons.chat_bubble_outline),
      title: Text(s.roomChat(chat.length)),
      children: [
        ConstrainedBox(
          constraints: const BoxConstraints(maxHeight: 280),
          child: ListView(
            shrinkWrap: true,
            reverse: true,
            children: [
              for (final m in chat.reversed)
                ListTile(
                  key: Key('roomMessage-${m.id}'),
                  dense: true,
                  leading: m.fromStage
                      ? const Icon(Icons.shield_outlined, size: 16)
                      : null,
                  title: Text(m.text),
                  subtitle: Text(
                    [
                      m.mine ? s.roomYou : (m.sender ?? '—'),
                      if (m.toHosts) s.roomChatToHosts,
                    ].join(' · '),
                    style: theme.textTheme.bodySmall,
                  ),
                  trailing: room.moderates && !m.mine
                      ? PopupMenuButton<String>(
                          key: Key('roomMessageMenu-${m.id}'),
                          onSelected: (what) => what == 'delete'
                              ? _say(
                                  widget.model,
                                  widget.call,
                                  AudienceAction.deleteMessage,
                                  id: m.id,
                                )
                              : _control(
                                  widget.model,
                                  widget.call,
                                  RoomControl.muteChat,
                                  member: m.senderId,
                                  on: true,
                                ),
                          itemBuilder: (_) => [
                            PopupMenuItem(
                              value: 'delete',
                              child: Text(s.roomChatDelete),
                            ),
                            if (room.host)
                              PopupMenuItem(
                                value: 'mute',
                                child: Text(s.roomChatMuteSender),
                              ),
                          ],
                        )
                      : null,
                ),
            ],
          ),
        ),
        if (room.chatMuted)
          ListTile(
            key: const Key('roomChatMuted'),
            dense: true,
            leading: Icon(
              Icons.speaker_notes_off,
              color: theme.colorScheme.error,
            ),
            title: Text(s.roomChatMuted),
          )
        else
          Padding(
            padding: const EdgeInsets.fromLTRB(16, 4, 8, 8),
            child: Row(
              children: [
                Expanded(
                  child: TextField(
                    key: const Key('roomChatField'),
                    controller: _text,
                    maxLength: 1000,
                    decoration: InputDecoration(
                      hintText: s.roomChatHint,
                      counterText: '',
                    ),
                  ),
                ),
                if (!room.moderates)
                  Tooltip(
                    message: s.roomChatToHosts,
                    child: Checkbox(
                      key: const Key('roomChatToHosts'),
                      value: _toHosts,
                      onChanged: (v) => setState(() => _toHosts = v ?? false),
                    ),
                  ),
                IconButton(
                  key: const Key('roomChatSend'),
                  icon: const Icon(Icons.send),
                  tooltip: s.roomSend,
                  onPressed: () async {
                    final text = _text.text.trim();
                    if (text.isEmpty) return;
                    _text.clear();
                    await _say(
                      widget.model,
                      widget.call,
                      AudienceAction.chat,
                      text: text,
                      on: _toHosts,
                    );
                  },
                ),
              ],
            ),
          ),
      ],
    );
  }
}
