// ignore_for_file: avoid_web_libraries_in_flutter, deprecated_member_use

import 'dart:async';
import 'dart:html';

import 'sse_message.dart';

class SseClient {
  const SseClient();

  Stream<BackendStreamMessage> connect(Uri uri) {
    late final StreamController<BackendStreamMessage> controller;
    late final EventSource source;

    void listenFor(String eventType) {
      source.addEventListener(eventType, (event) {
        if (event is MessageEvent) {
          controller.add(
            BackendStreamMessage(
              type: eventType,
              data: event.data?.toString() ?? '',
            ),
          );
        }
      });
    }

    controller = StreamController<BackendStreamMessage>(
      onListen: () {
        source = EventSource(uri.toString());
        for (final eventType in const [
          'session.started',
          'event.upsert',
          'bridge.upsert',
          'session.message',
          'session.done',
          'session.error',
        ]) {
          listenFor(eventType);
        }
        source.onError.listen((event) {
          controller.addError(StateError('SSE connection failed.'));
          source.close();
          controller.close();
        });
      },
      onCancel: () {
        source.close();
      },
    );

    return controller.stream;
  }
}
