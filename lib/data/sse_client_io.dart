import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'sse_message.dart';

class SseClient {
  const SseClient();

  Stream<BackendStreamMessage> connect(Uri uri) {
    late final StreamController<BackendStreamMessage> controller;
    HttpClientRequest? request;

    controller = StreamController<BackendStreamMessage>(
      onListen: () async {
        var eventType = 'message';
        final dataLines = <String>[];
        try {
          final client = HttpClient();
          request = await client.getUrl(uri);
          request!.headers.set(HttpHeaders.acceptHeader, 'text/event-stream');
          final response = await request!.close();
          if (response.statusCode < 200 || response.statusCode >= 300) {
            throw HttpException(
              'SSE endpoint returned HTTP ${response.statusCode}.',
              uri: uri,
            );
          }

          void dispatch() {
            if (dataLines.isEmpty) {
              eventType = 'message';
              return;
            }
            controller.add(
              BackendStreamMessage(
                type: eventType,
                data: dataLines.join('\n'),
              ),
            );
            eventType = 'message';
            dataLines.clear();
          }

          await for (final line in response
              .transform(utf8.decoder)
              .transform(const LineSplitter())) {
            if (line.isEmpty) {
              dispatch();
              continue;
            }
            if (line.startsWith(':')) {
              continue;
            }
            if (line.startsWith('event:')) {
              eventType = line.substring('event:'.length).trim();
              continue;
            }
            if (line.startsWith('data:')) {
              dataLines.add(line.substring('data:'.length).trimLeft());
            }
          }
          dispatch();
          await controller.close();
          client.close();
        } catch (error, stackTrace) {
          if (!controller.isClosed) {
            controller.addError(error, stackTrace);
            await controller.close();
          }
        }
      },
      onCancel: () {
        request?.abort();
      },
    );

    return controller.stream;
  }
}
