import 'dart:io';

import 'package:ai_news_canvas/data/research_session_client_io.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('surfaces non-json transcription errors without a parse failure',
      () async {
    final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(() => server.close(force: true));
    server.listen((request) async {
      await request.drain<void>();
      request.response
        ..statusCode = HttpStatus.internalServerError
        ..headers.contentType = ContentType.text
        ..write('Internal Server Error');
      await request.response.close();
    });

    final recording = await File(
      '${Directory.systemTemp.path}/ai-news-client-test.wav',
    ).writeAsBytes(<int>[0x52, 0x49, 0x46, 0x46]);
    addTearDown(() async {
      if (await recording.exists()) {
        await recording.delete();
      }
    });

    final client = ResearchSessionClient(
      baseUri: Uri.parse('http://127.0.0.1:${server.port}/graph/stream'),
    );

    await expectLater(
      client.transcribeRecording(recording.path),
      throwsA(
        isA<StateError>().having(
          (error) => error.toString(),
          'message',
          allOf(
            contains('Transcription failed (500)'),
            contains('Internal Server Error'),
          ),
        ),
      ),
    );
  });
}
