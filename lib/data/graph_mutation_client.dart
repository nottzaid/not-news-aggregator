import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;

import '../models/research_event.dart';
import 'graph_repository.dart';

class GraphMutationSnapshot {
  const GraphMutationSnapshot({
    required this.events,
    required this.bridges,
    required this.placements,
    required this.revision,
  });

  final List<ResearchEvent> events;
  final List<EventBridge> bridges;
  final Map<String, CanvasPlacement> placements;
  final int revision;

  factory GraphMutationSnapshot.fromJson(Map<String, Object?> json) {
    final placements = (json['placements'] as Map).cast<String, Object?>();
    return GraphMutationSnapshot(
      events: (json['events'] as List)
          .map((value) =>
              ResearchEvent.fromJson((value as Map).cast<String, Object?>()))
          .toList(growable: false),
      bridges: (json['bridges'] as List)
          .map((value) =>
              EventBridge.fromJson((value as Map).cast<String, Object?>()))
          .toList(growable: false),
      placements: {
        for (final entry in placements.entries)
          entry.key: CanvasPlacement.fromJson(
              (entry.value as Map).cast<String, Object?>()),
      },
      revision: (json['revision'] as num).toInt(),
    );
  }
}

class DragTransactionResult {
  const DragTransactionResult({
    required this.id,
    required this.status,
    required this.eventId,
    required this.originX,
    required this.originY,
    required this.snapshot,
  });

  final String id;
  final String status;
  final String eventId;
  final double originX;
  final double originY;
  final GraphMutationSnapshot snapshot;

  bool get settled =>
      status == 'resolved' || status == 'fallback' || status == 'undone';

  factory DragTransactionResult.fromJson(Map<String, Object?> json) {
    final origin = (json['origin'] as Map).cast<String, Object?>();
    return DragTransactionResult(
      id: json['id'] as String,
      status: json['status'] as String,
      eventId: json['eventId'] as String,
      originX: (origin['x'] as num).toDouble(),
      originY: (origin['y'] as num).toDouble(),
      snapshot: GraphMutationSnapshot.fromJson(
          (json['graph'] as Map).cast<String, Object?>()),
    );
  }
}

class GraphMutationClient {
  const GraphMutationClient({this.baseUri});

  final Uri? baseUri;

  Uri get _root =>
      (baseUri ?? Uri.parse(defaultGraphStreamUri)).replace(path: '/graph');

  Future<DragTransactionResult> drag({
    required String eventId,
    required double originX,
    required double originY,
    required double destinationX,
    required double destinationY,
    required String? targetEventId,
    required int expectedRevision,
  }) async {
    final response = await http.post(
      _root.replace(path: '/graph/drag-transactions'),
      headers: {'content-type': 'application/json'},
      body: jsonEncode({
        'eventId': eventId,
        'originX': originX,
        'originY': originY,
        'destinationX': destinationX,
        'destinationY': destinationY,
        if (targetEventId != null) 'targetEventId': targetEventId,
        'expectedRevision': expectedRevision,
      }),
    );
    return _decode(response, 'Drag');
  }

  Future<DragTransactionResult> get(String id) async {
    final response =
        await http.get(_root.replace(path: '/graph/drag-transactions/$id'));
    return _decode(response, 'Reconciliation');
  }

  Future<DragTransactionResult> undo(String id) async {
    final response = await http
        .post(_root.replace(path: '/graph/drag-transactions/$id/undo'));
    return _decode(response, 'Undo');
  }

  Future<String> review(String id) async {
    final response = await http
        .post(_root.replace(path: '/graph/drag-transactions/$id/review'));
    final decoded = jsonDecode(response.body);
    if (response.statusCode < 200 || response.statusCode >= 300) {
      final detail =
          decoded is Map ? decoded['detail']?.toString() : response.body.trim();
      throw StateError('Hermes review failed: $detail');
    }
    final review = (decoded as Map).cast<String, Object?>();
    return review['summary']?.toString() ?? 'Hermes returned no review.';
  }

  Future<DragTransactionResult> waitUntilSettled(String id) async {
    while (true) {
      await Future<void>.delayed(const Duration(milliseconds: 240));
      final result = await get(id);
      if (result.settled) {
        return result;
      }
    }
  }
}

DragTransactionResult _decode(http.Response response, String operation) {
  final decoded = jsonDecode(response.body);
  if (response.statusCode < 200 || response.statusCode >= 300) {
    final detail =
        decoded is Map ? decoded['detail']?.toString() : response.body.trim();
    throw StateError('$operation failed: ${detail ?? response.statusCode}');
  }
  return DragTransactionResult.fromJson(
      (decoded as Map).cast<String, Object?>());
}
