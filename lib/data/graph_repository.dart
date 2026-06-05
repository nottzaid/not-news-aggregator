import 'dart:async';
import 'dart:convert';

import 'package:http/http.dart' as http;

import '../models/research_event.dart';
import 'sse_client.dart';
import 'sse_message.dart';

typedef BackendStreamFactory = Stream<BackendStreamMessage> Function(Uri uri);
typedef BackendClearFactory = Future<void> Function(Uri uri);

const defaultGraphStreamUri = String.fromEnvironment(
  'AI_NEWS_GRAPH_STREAM_URL',
  defaultValue: 'http://127.0.0.1:8765/graph/stream',
);

class CanvasGraphState {
  const CanvasGraphState({
    required this.events,
    required this.bridges,
    required this.usingFallback,
    required this.isRunning,
    required this.progressMessages,
    this.error,
    this.message,
  });

  final List<ResearchEvent> events;
  final List<EventBridge> bridges;
  final bool usingFallback;
  final bool isRunning;
  final List<String> progressMessages;
  final String? error;
  final String? message;
}

class CanvasGraphRepository {
  CanvasGraphRepository({
    Uri? streamUri,
    BackendStreamFactory? streamFactory,
    BackendClearFactory? clearFactory,
    List<ResearchEvent> fallbackEvents = const [],
    List<EventBridge> fallbackBridges = const [],
  })  : streamUri = streamUri ?? Uri.parse(defaultGraphStreamUri),
        _streamFactory = streamFactory ?? const SseClient().connect,
        _clearFactory = clearFactory ?? _deleteGraph,
        _fallbackEvents = fallbackEvents,
        _fallbackBridges = fallbackBridges;

  final Uri streamUri;
  final BackendStreamFactory _streamFactory;
  final BackendClearFactory _clearFactory;
  final List<ResearchEvent> _fallbackEvents;
  final List<EventBridge> _fallbackBridges;

  Stream<CanvasGraphState> watch({
    Uri? uri,
    bool startsSession = false,
    List<ResearchEvent>? initialEvents,
    List<EventBridge>? initialBridges,
  }) async* {
    final progressMessages =
        startsSession ? <String>['Starting research session...'] : <String>[];
    var state = CanvasGraphState(
      events: initialEvents ?? _fallbackEvents,
      bridges: initialBridges ?? _fallbackBridges,
      usingFallback: initialEvents == null,
      isRunning: startsSession,
      progressMessages: progressMessages,
      message: startsSession ? 'Starting research session...' : null,
    );
    yield state;

    try {
      await for (final message in _streamFactory(uri ?? streamUri)) {
        final next = _applyMessage(state, message);
        if (next != null) {
          state = next;
          yield state;
        }
        if (message.type == 'session.done' || message.type == 'session.error') {
          break;
        }
      }
    } catch (error) {
      yield CanvasGraphState(
        events: state.events,
        bridges: state.bridges,
        usingFallback: state.usingFallback,
        isRunning: false,
        progressMessages: state.progressMessages,
        error: error.toString(),
        message: state.message,
      );
    }
  }

  CanvasGraphState? _applyMessage(
      CanvasGraphState state, BackendStreamMessage message) {
    return switch (message.type) {
      'event.upsert' => _upsertEvent(
          state, ResearchEvent.fromJson(_decodeObject(message.data))),
      'bridge.upsert' =>
        _upsertBridge(state, EventBridge.fromJson(_decodeObject(message.data))),
      'session.started' =>
        _withProgressMessage(state, message, isRunning: true),
      'session.done' => _finishSession(state, message),
      'session.error' => _finishSession(state, message, isError: true),
      'session.message' =>
        _withProgressMessage(state, message, isRunning: true),
      _ => null,
    };
  }

  Future<void> clear() {
    return _clearFactory(_graphUriFromStream(streamUri));
  }

  CanvasGraphState _upsertEvent(CanvasGraphState state, ResearchEvent event) {
    final events = state.usingFallback ? <ResearchEvent>[] : [...state.events];
    final bridges = state.usingFallback ? <EventBridge>[] : state.bridges;
    final index = events.indexWhere((candidate) => candidate.id == event.id);
    if (index == -1) {
      events.add(event);
    } else {
      events[index] = event;
    }
    return CanvasGraphState(
      events: events,
      bridges: bridges,
      usingFallback: false,
      isRunning: state.isRunning,
      progressMessages: state.progressMessages,
      message: state.message,
    );
  }

  CanvasGraphState _upsertBridge(CanvasGraphState state, EventBridge bridge) {
    final events =
        state.usingFallback ? <ResearchEvent>[...state.events] : state.events;
    final bridges = state.usingFallback ? <EventBridge>[] : [...state.bridges];
    final index = bridges.indexWhere(
      (candidate) =>
          candidate.from == bridge.from &&
          candidate.to == bridge.to &&
          candidate.label == bridge.label,
    );
    if (index == -1) {
      bridges.add(bridge);
    } else {
      bridges[index] = bridge;
    }
    return CanvasGraphState(
      events: events,
      bridges: bridges,
      usingFallback: false,
      isRunning: state.isRunning,
      progressMessages: state.progressMessages,
      message: state.message,
    );
  }

  CanvasGraphState _withProgressMessage(
    CanvasGraphState state,
    BackendStreamMessage message, {
    required bool isRunning,
  }) {
    final decodedMessage = _decodeMessage(message.data);
    return CanvasGraphState(
      events: state.events,
      bridges: state.bridges,
      usingFallback: state.usingFallback,
      isRunning: isRunning,
      progressMessages: _appendMessage(state.progressMessages, decodedMessage),
      message: decodedMessage,
    );
  }

  CanvasGraphState _finishSession(
    CanvasGraphState state,
    BackendStreamMessage message, {
    bool isError = false,
  }) {
    final decodedMessage = _decodeMessage(message.data);
    return CanvasGraphState(
      events: state.events,
      bridges: state.bridges,
      usingFallback: isError ? state.usingFallback : false,
      isRunning: false,
      progressMessages: _appendMessage(state.progressMessages, decodedMessage),
      error: isError ? decodedMessage : null,
      message: decodedMessage,
    );
  }
}

Uri _graphUriFromStream(Uri streamUri) {
  return Uri(
    scheme: streamUri.scheme,
    userInfo: streamUri.userInfo,
    host: streamUri.host,
    port: streamUri.hasPort ? streamUri.port : null,
    path: '/graph',
  );
}

Future<void> _deleteGraph(Uri uri) async {
  final response = await http.delete(uri).timeout(const Duration(seconds: 12));
  if (response.statusCode < 200 || response.statusCode >= 300) {
    final detail = response.body.trim();
    throw StateError(
      detail.isEmpty
          ? 'Clear failed (${response.statusCode}).'
          : 'Clear failed (${response.statusCode}): $detail',
    );
  }
}

List<String> _appendMessage(List<String> messages, String message) {
  final trimmed = message.trim();
  if (trimmed.isEmpty || (messages.isNotEmpty && messages.last == trimmed)) {
    return messages;
  }
  const maxMessages = 80;
  final next = [...messages, trimmed];
  if (next.length <= maxMessages) {
    return next;
  }
  return next.sublist(next.length - maxMessages);
}

Map<String, Object?> _decodeObject(String data) {
  final decoded = jsonDecode(data);
  if (decoded is Map<String, Object?>) {
    return decoded;
  }
  if (decoded is Map) {
    return decoded.cast<String, Object?>();
  }
  throw const FormatException('Expected SSE data payload to be a JSON object.');
}

String _decodeMessage(String data) {
  try {
    final decoded = _decodeObject(data);
    final message = decoded['message'];
    if (message is String && message.isNotEmpty) {
      return message;
    }
  } on FormatException {
    if (data.isNotEmpty) {
      return data;
    }
  }
  return 'Research session updated.';
}
