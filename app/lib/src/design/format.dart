// Times and lengths as the screens show them.

import 'package:clock/clock.dart';
import 'package:flutter/material.dart';
import 'package:intl/intl.dart';

import '../../l10n/app_localizations.dart';
import '../phone/phone_api.dart';

/// `4:07`, or `1:02:03` past the hour.
String duration(int secs) {
  final h = secs ~/ 3600;
  final m = (secs % 3600) ~/ 60;
  final s = (secs % 60).toString().padLeft(2, '0');
  return h > 0 ? '$h:${m.toString().padLeft(2, '0')}:$s' : '$m:$s';
}

/// When something happened: the time today, "Yesterday", else the date.
String when(BuildContext context, DateTime? at, {DateTime? now}) {
  if (at == null) return '';
  final locale = Localizations.localeOf(context).toString();
  final today = DateUtils.dateOnly(now ?? clock.now());
  final day = DateUtils.dateOnly(at);
  if (day == today) return DateFormat.jm(locale).format(at);
  if (day == today.subtract(const Duration(days: 1))) {
    return Strings.of(context).yesterday;
  }
  return DateFormat.MMMd(locale).format(at);
}

/// A presence as a person reads it.
String presenceLabel(Strings s, Person p) {
  if (p.onCall) return s.onACall;
  return switch (p.presence) {
    'available' || 'online' || 'open' => s.presenceAvailable,
    'away' || 'idle' || 'brb' => s.presenceAway,
    'busy' || 'on-the-phone' => s.presenceBusy,
    'dnd' => s.presenceDnd,
    null => '',
    _ => s.presenceOffline,
  };
}
