import 'dart:convert';

import 'package:ai_news_canvas/data/fixture_events.dart';
import 'package:ai_news_canvas/data/graph_repository.dart';
import 'package:ai_news_canvas/data/sse_message.dart';
import 'package:ai_news_canvas/models/research_event.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('parses backend event and bridge DTOs', () {
    final event = ResearchEvent.fromJson({
      'id': 'single-source',
      'title': 'Single source event',
      'date': 'Jun 3, 2026',
      'color': 0xff123456,
      'summary': 'A one-source event should remain a point.',
      'sourceLabel': 'Official',
      'artifacts': <Object?>[],
      'url': 'https://example.com/story',
    });
    final bridge = EventBridge.fromJson({
      'from': 'single-source',
      'to': 'spacex',
      'label': 'related strategy',
    });

    expect(event.canExpand, isFalse);
    expect(event.directUrl, 'https://example.com/story');
    expect(event.toJson()['sourceLabel'], 'Official');
    expect(bridge.from, 'single-source');
    expect(bridge.toJson()['from'], 'single-source');
  });

  test('keeps fixture graph as fallback when backend stream fails', () async {
    final repository = CanvasGraphRepository(
      fallbackEvents: fixtureEvents,
      fallbackBridges: fixtureBridges,
      streamFactory: (_) => Stream.error(StateError('backend unavailable')),
    );

    final states = await repository.watch().toList();

    expect(states.first.usingFallback, isTrue);
    expect(states.first.events, fixtureEvents);
    expect(states.last.usingFallback, isTrue);
    expect(states.last.error, contains('backend unavailable'));
  });

  test('empty backend stream leaves the canvas empty', () async {
    final repository = CanvasGraphRepository(
      streamFactory: (_) => Stream.fromIterable([
        const BackendStreamMessage(
          type: 'session.message',
          data: '{"message":"Canvas is empty."}',
        ),
        const BackendStreamMessage(
          type: 'session.done',
          data: '{"message":"Empty canvas loaded."}',
        ),
      ]),
    );

    final states = await repository.watch().toList();

    expect(states.last.usingFallback, isFalse);
    expect(states.last.events, isEmpty);
    expect(states.last.bridges, isEmpty);
    expect(states.last.message, 'Empty canvas loaded.');
  });

  test('clear calls the backend graph endpoint', () async {
    Uri? clearedUri;
    final repository = CanvasGraphRepository(
      streamUri: Uri.parse('http://127.0.0.1:8765/graph/stream'),
      clearFactory: (uri) async => clearedUri = uri,
    );

    await repository.clear();

    expect(clearedUri, Uri.parse('http://127.0.0.1:8765/graph'));
  });

  test('applies event and bridge upsert mutations from SSE', () async {
    const streamedEvent = ResearchEvent(
      id: 'new-event',
      title: 'New event',
      date: 'Jun 4, 2026',
      color: 0xff445566,
      summary: 'A streamed event.',
      sourceLabel: 'Backend',
      artifacts: [
        SourceArtifact(
          text: 'Backend source',
          source: 'official',
          url: 'https://example.com/backend',
        ),
      ],
    );
    const streamedBridge = EventBridge(
      from: 'new-event',
      to: 'spacex',
      label: 'streamed relationship',
    );
    final repository = CanvasGraphRepository(
      streamFactory: (_) => Stream.fromIterable([
        BackendStreamMessage(
            type: 'event.upsert', data: jsonEncode(streamedEvent.toJson())),
        BackendStreamMessage(
            type: 'bridge.upsert', data: jsonEncode(streamedBridge.toJson())),
        const BackendStreamMessage(type: 'session.done', data: '{}'),
      ]),
    );

    final states = await repository.watch().toList();

    expect(states.last.usingFallback, isFalse);
    expect(states.last.events.any((event) => event.id == 'new-event'), isTrue);
    expect(states.last.bridges.any((bridge) => bridge.from == 'new-event'),
        isTrue);
    expect(
        states.last.events
            .firstWhere((event) => event.id == 'new-event')
            .canExpand,
        isFalse);
    expect(
      states.last.events
          .firstWhere((event) => event.id == 'new-event')
          .directUrl,
      'https://example.com/backend',
    );
  });

  test('research stream extends the existing canvas graph', () async {
    final existing = fixtureEvents.first;
    const streamedEvent = ResearchEvent(
      id: 'cosmos-new',
      title: 'Cosmos New',
      date: 'Jun 4, 2026',
      color: 0xff76b900,
      summary: 'A new streamed event.',
      sourceLabel: 'Backend',
      artifacts: [
        SourceArtifact(
          text: 'NVIDIA source',
          source: 'official',
          url: 'https://example.com/cosmos',
        ),
      ],
    );
    final repository = CanvasGraphRepository(
      streamFactory: (_) => Stream.fromIterable([
        BackendStreamMessage(
            type: 'event.upsert', data: jsonEncode(streamedEvent.toJson())),
        const BackendStreamMessage(type: 'session.done', data: '{}'),
      ]),
    );

    final states = await repository.watch(
      startsSession: true,
      initialEvents: [existing],
      initialBridges: const [],
    ).toList();

    expect(states.last.events.map((event) => event.id), contains(existing.id));
    expect(states.last.events.map((event) => event.id),
        contains(streamedEvent.id));
    expect(states.last.usingFallback, isFalse);
  });
}
