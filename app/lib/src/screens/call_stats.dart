// A call's stats while it is up (FCP's `docs/CALL_DIAGNOSTICS.md`, D8
// finding 43): what is sent and received, loss, jitter, what the far end
// reports over RTCP, the round trip, and the estimated score each way,
// updated every second. A dialog on a wide window, a sheet on a narrow one.

import 'package:flutter/material.dart';

import '../../l10n/app_localizations.dart';
import '../design/theme.dart';
import '../phone/phone_api.dart';
import '../phone/phone_model.dart';

/// Below this width the stats open as a sheet.
const _sheetBelow = 600.0;

/// Open the stats of call [id]; they close themselves when it ends.
Future<void> showCallStats(BuildContext context, PhoneModel model, int id) {
  Widget body(BuildContext context) => _CallStats(model: model, id: id);
  if (MediaQuery.sizeOf(context).width < _sheetBelow) {
    return showModalBottomSheet<void>(
      context: context,
      showDragHandle: true,
      isScrollControlled: true,
      builder: (sheet) => DraggableScrollableSheet(
        expand: false,
        initialChildSize: 0.7,
        maxChildSize: 0.95,
        builder: (sheet, scroll) =>
            SingleChildScrollView(controller: scroll, child: body(sheet)),
      ),
    );
  }
  return showDialog<void>(
    context: context,
    builder: (dialog) => Dialog(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 480),
        child: SingleChildScrollView(child: body(dialog)),
      ),
    ),
  );
}

/// The colour and words of a level.
(Color, String) levelLook(Strings s, QualityLevel level) => switch (level) {
  QualityLevel.good => (AnvilColors.answer, s.qualityGood),
  QualityLevel.fair => (AnvilColors.held, s.qualityFair),
  QualityLevel.poor => (AnvilColors.hangup, s.qualityPoor),
};

String _score(int hundredths) => (hundredths / 100).toStringAsFixed(1);

String _pct(int permille) => (permille / 10).toStringAsFixed(1);

class _CallStats extends StatelessWidget {
  const _CallStats({required this.model, required this.id});
  final PhoneModel model;
  final int id;

  @override
  Widget build(BuildContext context) => ListenableBuilder(
    listenable: model,
    builder: (context, _) {
      final call = model.snapshot.calls.where((c) => c.id == id).firstOrNull;
      if (call == null) {
        // The call is over: so are its stats.
        WidgetsBinding.instance.addPostFrameCallback((_) {
          if (context.mounted) Navigator.of(context).maybePop();
        });
        return const SizedBox.shrink();
      }
      return _Body(call: call);
    },
  );
}

class _Body extends StatelessWidget {
  const _Body({required this.call});
  final PhoneCall call;

  @override
  Widget build(BuildContext context) {
    final s = Strings.of(context);
    final theme = Theme.of(context);
    final q = call.quality;
    final rows = <Widget>[];
    void row(String key, String label, String value) =>
        rows.add(_Row(key: Key('stat-$key'), label: label, value: value));

    if (q == null) {
      row('waiting', s.callStats, s.statsWaiting);
    } else {
      row('youHear', s.statsYouHear, _score(q.mosHundredths));
      row(
        'theyHear',
        s.statsTheyHear,
        q.farMosHundredths == null
            ? s.statsUnknown
            : _score(q.farMosHundredths!),
      );
      row(
        'audio',
        s.statsAudio,
        s.statsCodecValue(
          (call.codec ?? '').toUpperCase(),
          (q.clockRate / 1000).toStringAsFixed(q.clockRate % 1000 == 0 ? 0 : 1),
          '${q.ptimeMs}',
        ),
      );
      row(
        'encryption',
        s.statsEncryption,
        call.encrypted ? s.statsSrtp : s.statsNone,
      );
      if (q.remoteRtp case final remote?) {
        row('media', s.statsMedia, '${q.localRtp ?? '?'} ↔ $remote');
      }
      row(
        'sent',
        s.statsSent,
        '${s.statsRate('${q.sendPps}', '${q.sendKbps}')}\n'
            '${s.statsTotal('${q.packetsSent}', '${q.bytesSent ~/ 1000}')}',
      );
      row(
        'received',
        s.statsReceived,
        '${s.statsRate('${q.recvPps}', '${q.recvKbps}')}\n'
            '${s.statsTotal('${q.rtpReceived}', '${q.bytesReceived ~/ 1000}')}',
      );
      row(
        'loss',
        s.statsLoss,
        s.statsLossValue(
          _pct(q.recentLossPermille),
          _pct(q.lossPermille),
          '${q.packetsLost}',
          '${q.packetsExpected}',
        ),
      );
      row('jitter', s.statsJitter, s.statsMs('${q.jitterMs}'));
      if (q.packetsDropped > 0) {
        row(
          'dropped',
          s.statsDropped,
          [
            s.qualityPackets(
              q.packetsReceived,
              q.packetsPlayed,
              q.packetsDropped,
            ),
            if ((q.unexpectedPt, q.expectedPt) case (
              final pt?,
              final expected?,
            ))
              s.qualityUnexpectedPt(pt, expected),
          ].join('\n'),
        );
      }
      row(
        'farEnd',
        s.statsFarEnd,
        !q.rtcp
            ? s.statsNoRtcp
            : q.farLossPermille == null
            ? s.statsNoReports
            : s.statsFarValue(
                _pct(q.farLossPermille!),
                '${q.farJitterMs ?? 0}',
                '${q.farLost ?? 0}',
              ),
      );
      row(
        'roundTrip',
        s.statsRoundTrip,
        q.rttMs == null ? s.statsUnknown : s.statsMs('${q.rttMs}'),
      );
      if (q.rtcp) {
        row(
          'rtcp',
          s.statsRtcp,
          s.statsRtcpValue('${q.rtcpSent}', '${q.rtcpReceived}'),
        );
      }
    }

    final level = q?.mos == null ? null : levelLook(s, q!.level);
    return Padding(
      key: const Key('callStats'),
      padding: const EdgeInsets.fromLTRB(24, 16, 24, 24),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Text(s.callStats, style: theme.textTheme.titleLarge),
              ),
              if (level case (final color, final words))
                Text(
                  '${q!.mos!.toStringAsFixed(1)} · $words',
                  key: const Key('statsScore'),
                  style: theme.textTheme.titleMedium?.copyWith(color: color),
                ),
            ],
          ),
          const SizedBox(height: 4),
          Text(s.statsScoreHint, style: theme.textTheme.bodySmall),
          const SizedBox(height: 12),
          ...rows,
        ],
      ),
    );
  }
}

class _Row extends StatelessWidget {
  const _Row({super.key, required this.label, required this.value});
  final String label;
  final String value;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          SizedBox(
            width: 140,
            child: Text(
              label,
              style: theme.textTheme.bodyMedium?.copyWith(
                color: theme.colorScheme.onSurfaceVariant,
              ),
            ),
          ),
          Expanded(child: SelectableText(value)),
        ],
      ),
    );
  }
}
