// The attendance report of one of the user's own meetings (FCP's
// `docs/WEBINARS.md` W8.3): who came to a run of its room, how long, which
// registrations came or watched the stream, picked among its runs.

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

/// A meeting's attendance, in a sheet.
class AttendanceSheet extends StatefulWidget {
  const AttendanceSheet({
    super.key,
    required this.model,
    required this.meeting,
  });
  final PhoneModel model;
  final Meeting meeting;

  static Future<void> show(
    BuildContext context,
    PhoneModel model,
    Meeting meeting,
  ) => showModalBottomSheet<void>(
    context: context,
    isScrollControlled: true,
    builder: (_) => AttendanceSheet(model: model, meeting: meeting),
  );

  @override
  State<AttendanceSheet> createState() => _AttendanceSheetState();
}

class _AttendanceSheetState extends State<AttendanceSheet> {
  Future<AttendanceReport>? _report;
  String? _sitting;

  @override
  void initState() {
    super.initState();
    _load();
  }

  void _load() {
    setState(() {
      _report = widget.model.api.meetingAttendance(
        widget.meeting.id,
        sitting: _sitting,
      );
    });
  }

  String _when(BuildContext context, DateTime t) {
    final l = MaterialLocalizations.of(context);
    return '${l.formatMediumDate(t)} ${l.formatTimeOfDay(TimeOfDay.fromDateTime(t))}';
  }

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final theme = Theme.of(context);
    int minutes(int secs) => (secs / 60).round();
    return SafeArea(
      child: Padding(
        key: const Key('attendanceSheet'),
        padding: const EdgeInsets.fromLTRB(16, 16, 16, 8),
        child: FutureBuilder<AttendanceReport>(
          future: _report,
          builder: (context, snap) {
            if (snap.hasError) {
              final error = snap.error.toString();
              return Text(
                error.contains('404')
                    ? s.attendanceNone
                    : s.attendanceError(error),
                key: const Key('attendanceNone'),
              );
            }
            final r = snap.data;
            if (r == null) {
              return const Center(child: CircularProgressIndicator());
            }
            return ConstrainedBox(
              constraints: BoxConstraints(
                maxHeight: MediaQuery.of(context).size.height * 0.8,
              ),
              child: ListView(
                shrinkWrap: true,
                children: [
                  Text(
                    s.attendanceTitle(widget.meeting.name),
                    style: theme.textTheme.titleLarge,
                  ),
                  if (r.sittings.length > 1)
                    DropdownButton<String>(
                      key: const Key('attendanceSitting'),
                      value: r.sitting.id,
                      items: [
                        for (final x in r.sittings)
                          DropdownMenuItem(
                            value: x.id,
                            child: Text(
                              '${s.attendanceRun}: ${_when(context, x.startedAt)}',
                            ),
                          ),
                      ],
                      onChanged: (id) {
                        _sitting = id;
                        _load();
                      },
                    )
                  else
                    Text(
                      _when(context, r.sitting.startedAt),
                      style: theme.textTheme.bodySmall,
                    ),
                  const SizedBox(height: 8),
                  Text(
                    s.attendanceSummary(r.attendees, r.peak),
                    key: const Key('attendanceSummary'),
                  ),
                  const Divider(),
                  Text(s.attendancePeople, style: theme.textTheme.titleSmall),
                  for (final l in r.legs)
                    ListTile(
                      dense: true,
                      title: Text(l.name),
                      subtitle: Text(
                        [
                          if (l.role != null) l.role!,
                          _when(context, l.joinedAt),
                          if (l.lobby)
                            s.attendanceLobbyOnly
                          else
                            s.attendanceMinutes(minutes(l.durationSecs)),
                        ].join(' · '),
                      ),
                    ),
                  if (r.registrants.isNotEmpty) ...[
                    const Divider(),
                    Text(
                      s.attendanceRegistrants,
                      style: theme.textTheme.titleSmall,
                    ),
                    for (final x in r.registrants)
                      ListTile(
                        dense: true,
                        title: Text(x.name),
                        subtitle: Text(
                          [
                            if (x.email != null) x.email!,
                            if (x.revoked) s.attendanceRevoked,
                            x.attended
                                ? s.attendanceCame(minutes(x.attendedSecs))
                                : s.attendanceDidNotCome,
                            if (x.watched)
                              x.watchedSecs > 0
                                  ? s.attendanceWatched(minutes(x.watchedSecs))
                                  : s.attendanceOpened,
                          ].join(' · '),
                        ),
                      ),
                  ],
                  if (r.viewers.isNotEmpty) ...[
                    const Divider(),
                    Text(
                      s.attendanceViewers,
                      style: theme.textTheme.titleSmall,
                    ),
                    for (final v in r.viewers)
                      ListTile(
                        dense: true,
                        title: Text(v.name),
                        subtitle: Text(
                          v.watchedSecs > 0
                              ? s.attendanceWatched(minutes(v.watchedSecs))
                              : s.attendanceOpened,
                        ),
                      ),
                  ],
                ],
              ),
            );
          },
        ),
      ),
    );
  }
}
