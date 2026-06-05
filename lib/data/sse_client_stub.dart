import 'sse_message.dart';

class SseClient {
  const SseClient();

  Stream<BackendStreamMessage> connect(Uri uri) {
    return const Stream.empty();
  }
}
