export 'sse_client_stub.dart'
    if (dart.library.html) 'sse_client_web.dart'
    if (dart.library.io) 'sse_client_io.dart';
