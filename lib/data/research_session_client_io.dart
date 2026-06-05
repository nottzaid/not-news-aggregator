import 'dart:convert';

import 'package:http/http.dart' as http;

import 'graph_repository.dart';

class ResearchSessionClient {
  const ResearchSessionClient({this.baseUri});

  final Uri? baseUri;

  Future<String> transcribeRecording(String path) async {
    final uri = (baseUri ?? Uri.parse(defaultGraphStreamUri))
        .replace(path: '/audio/transcribe');
    final request = http.MultipartRequest('POST', uri)
      ..files.add(await http.MultipartFile.fromPath('audio', path));
    final response = await request.send();
    final body = await response.stream.bytesToString();
    final decoded = _tryDecodeJson(body);
    if (response.statusCode < 200 || response.statusCode >= 300) {
      throw StateError(
        'Transcription failed (${response.statusCode}): '
        '${_responseMessage(decoded, body)}',
      );
    }
    if (decoded is Map &&
        decoded['text'] is String &&
        (decoded['text'] as String).trim().isNotEmpty) {
      return (decoded['text'] as String).trim();
    }
    throw StateError(
      'Transcription returned no text: ${_responseMessage(decoded, body)}',
    );
  }
}

Object? _tryDecodeJson(String body) {
  if (body.trim().isEmpty) {
    return null;
  }
  try {
    return jsonDecode(body);
  } on FormatException {
    return null;
  }
}

String _responseMessage(Object? decoded, String body) {
  if (decoded is Map) {
    for (final key in const ['error', 'detail', 'message']) {
      final value = decoded[key];
      if (value != null && value.toString().trim().isNotEmpty) {
        return value.toString();
      }
    }
  }
  final trimmed = body.trim();
  if (trimmed.isEmpty) {
    return 'empty response from server';
  }
  const maxLength = 300;
  if (trimmed.length <= maxLength) {
    return trimmed;
  }
  return '${trimmed.substring(0, maxLength)}...';
}
