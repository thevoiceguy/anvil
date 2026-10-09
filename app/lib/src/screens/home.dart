// The signed-in app: the keypad and calls, recents, people, voicemail and
// settings, with a call ringing in and the call up shown over every page.

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../design/theme.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';
import 'calls.dart';
import 'people.dart';
import 'recents.dart';
import 'settings.dart';
import 'voicemail.dart';
import '../update/update_banner.dart';

/// The pages, in the order the navigation shows them.
enum Section { phone, recents, people, voicemail, settings }

class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key, required this.model});
  final PhoneModel model;

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  Section _page = Section.phone;

  /// The keypad over a call up, to place a second one.
  bool _adding = false;

  PhoneModel get model => widget.model;

  /// Call someone from any page: the phone page then shows the call.
  void _dial(String target) {
    setState(() {
      _page = Section.phone;
      _adding = false;
    });
    model.run(() => model.api.call(target));
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final snap = model.snapshot;
    final ringing = snap.calls.where((c) => c.isRingingIn).toList();
    final up = snap.calls.where((c) => !c.isRingingIn).toList();
    if (up.isEmpty) _adding = false;

    final pages = <(Section, IconData, IconData, String, int)>[
      (Section.phone, Icons.dialpad_outlined, Icons.dialpad, s.navKeypad, 0),
      (
        Section.recents,
        Icons.history_outlined,
        Icons.history,
        s.navRecents,
        snap.missedCalls,
      ),
      (Section.people, Icons.people_outline, Icons.people, s.navPeople, 0),
      (
        Section.voicemail,
        Icons.voicemail_outlined,
        Icons.voicemail,
        s.navVoicemail,
        snap.newVoicemail,
      ),
      (
        Section.settings,
        Icons.settings_outlined,
        Icons.settings,
        s.navSettings,
        0,
      ),
    ];
    Widget badged(IconData icon, int count, Section page) => Badge(
      key: Key('badge-${page.name}'),
      isLabelVisible: count > 0,
      label: Text('$count'),
      child: Icon(icon),
    );

    final update = UpdateScope.maybeOf(context);
    final body = Column(
      children: [
        if (update != null)
          UpdateBanner(
            updates: update.updates,
            phone: model,
            quit: update.quit,
          ),
        for (final call in ringing) IncomingCallCard(model: model, call: call),
        if (up.isNotEmpty && _page != Section.phone)
          CallBar(
            model: model,
            onOpen: () => setState(() => _page = Section.phone),
          ),
        if (model.notice != null) _NoticeBanner(model: model),
        Expanded(child: _pageBody(up)),
      ],
    );

    return LayoutBuilder(
      builder: (context, constraints) {
        final wide = constraints.maxWidth >= 640;
        return Scaffold(
          appBar: _appBar(context),
          body: SafeArea(
            child: wide
                ? Row(
                    children: [
                      NavigationRail(
                        selectedIndex: _page.index,
                        labelType: NavigationRailLabelType.all,
                        onDestinationSelected: (i) =>
                            setState(() => _page = Section.values[i]),
                        destinations: [
                          for (final (page, icon, selected, label, count)
                              in pages)
                            NavigationRailDestination(
                              icon: badged(icon, count, page),
                              selectedIcon: Icon(selected),
                              label: Text(label),
                            ),
                        ],
                      ),
                      const VerticalDivider(width: 1),
                      Expanded(child: body),
                    ],
                  )
                : body,
          ),
          bottomNavigationBar: wide
              ? null
              : NavigationBar(
                  selectedIndex: _page.index,
                  onDestinationSelected: (i) =>
                      setState(() => _page = Section.values[i]),
                  destinations: [
                    for (final (page, icon, selected, label, count) in pages)
                      NavigationDestination(
                        icon: badged(icon, count, page),
                        selectedIcon: Icon(selected),
                        label: label,
                      ),
                  ],
                ),
        );
      },
    );
  }

  Widget _pageBody(List<PhoneCall> up) => switch (_page) {
    Section.phone =>
      up.isNotEmpty && !_adding
          ? InCallPanel(
              model: model,
              onAddCall: () => setState(() => _adding = true),
            )
          : Keypad(
              onCall: _dial,
              onBack: up.isEmpty ? null : () => setState(() => _adding = false),
            ),
    Section.recents => RecentsPage(model: model, onDial: _dial),
    Section.people => PeoplePage(model: model, onDial: _dial),
    Section.voicemail => VoicemailPage(model: model, onDial: _dial),
    Section.settings => SettingsPage(model: model),
  };

  PreferredSizeWidget _appBar(BuildContext context) {
    final s = Strings.of(context);
    final snap = model.snapshot;
    return AppBar(
      leading: snap.brandLogo == null
          ? null
          : Padding(
              padding: const EdgeInsets.all(8),
              child: Image.memory(
                snap.brandLogo!,
                key: const Key('brandLogo'),
                errorBuilder: (_, _, _) => const SizedBox.shrink(),
              ),
            ),
      title: Text(
        snap.brandName ?? model.account?.username ?? s.appName,
        key: const Key('title'),
      ),
      actions: [
        if (snap.dnd == true)
          Padding(
            padding: const EdgeInsets.symmetric(horizontal: 4),
            child: Chip(
              key: const Key('dndChip'),
              avatar: const Icon(Icons.do_not_disturb_on, size: 16),
              label: Text(s.dndOn),
            ),
          ),
        _RegistrationChip(registration: snap.registration),
        const SizedBox(width: 8),
      ],
    );
  }
}

class _NoticeBanner extends StatelessWidget {
  const _NoticeBanner({required this.model});
  final PhoneModel model;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final notice = model.notice!;
    final text = switch (notice.kind) {
      'call_ended' => s.callEnded(notice.text),
      'transfer' => s.transferProgress(notice.text),
      'microphone' => s.microphoneRefused,
      _ => s.commandFailed(notice.text),
    };
    return Material(
      color: Theme.of(context).colorScheme.surfaceContainerHighest,
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 4, 4, 4),
        child: Row(
          children: [
            Expanded(child: Text(text, key: const Key('notice'))),
            IconButton(
              icon: const Icon(Icons.close, size: 18),
              onPressed: model.clearNotice,
            ),
          ],
        ),
      ),
    );
  }
}

class _RegistrationChip extends StatelessWidget {
  const _RegistrationChip({required this.registration});
  final Registration registration;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final (label, color) = switch (registration) {
      Registration.registered => (s.registrationRegistered, AnvilColors.answer),
      Registration.registering => (s.registrationRegistering, Colors.amber),
      Registration.unregistered => (s.registrationUnregistered, Colors.grey),
      Registration.failed => (s.registrationFailed, AnvilColors.hangup),
    };
    return Chip(
      key: const Key('registration'),
      avatar: CircleAvatar(backgroundColor: color, radius: 5),
      label: Text(label),
    );
  }
}
