class ResearchEvent {
  const ResearchEvent({
    required this.id,
    required this.title,
    required this.date,
    required this.color,
    required this.summary,
    required this.sourceLabel,
    required this.artifacts,
    this.url,
  });

  final String id;
  final String title;
  final String date;
  final int color;
  final String summary;
  final String sourceLabel;
  final List<SourceArtifact> artifacts;
  final String? url;

  bool get canExpand => artifacts.length > 1;

  String? get directUrl {
    if (url != null) {
      return url;
    }
    if (artifacts.length == 1) {
      return artifacts.single.url;
    }
    return null;
  }

  factory ResearchEvent.fromJson(Map<String, Object?> json) {
    return ResearchEvent(
      id: _readString(json, 'id'),
      title: _readString(json, 'title'),
      date: _readString(json, 'date'),
      color: _readColor(json['color']),
      summary: _readString(json, 'summary'),
      sourceLabel: _readString(json, 'sourceLabel'),
      artifacts: _readList(json['artifacts'])
          .map((value) => SourceArtifact.fromJson(_readMap(value)))
          .toList(growable: false),
      url: json['url'] as String?,
    );
  }

  Map<String, Object?> toJson() {
    return {
      'id': id,
      'title': title,
      'date': date,
      'color': color,
      'summary': summary,
      'sourceLabel': sourceLabel,
      'artifacts': artifacts
          .map((artifact) => artifact.toJson())
          .toList(growable: false),
      if (url != null) 'url': url,
    };
  }
}

class SourceArtifact {
  const SourceArtifact({
    required this.text,
    required this.source,
    required this.url,
  });

  final String text;
  final String source;
  final String url;

  factory SourceArtifact.fromJson(Map<String, Object?> json) {
    return SourceArtifact(
      text: _readString(json, 'text'),
      source: _readString(json, 'source'),
      url: _readString(json, 'url'),
    );
  }

  Map<String, Object?> toJson() {
    return {
      'text': text,
      'source': source,
      'url': url,
    };
  }
}

class EventBridge {
  const EventBridge({
    required this.from,
    required this.to,
    required this.label,
  });

  final String from;
  final String to;
  final String label;

  factory EventBridge.fromJson(Map<String, Object?> json) {
    return EventBridge(
      from: _readString(json, 'from'),
      to: _readString(json, 'to'),
      label: _readString(json, 'label'),
    );
  }

  Map<String, Object?> toJson() {
    return {
      'from': from,
      'to': to,
      'label': label,
    };
  }
}

String _readString(Map<String, Object?> json, String key) {
  final value = json[key];
  if (value is String && value.isNotEmpty) {
    return value;
  }
  throw FormatException('Expected non-empty string at "$key".');
}

int _readColor(Object? value) {
  if (value is int) {
    return value;
  }
  if (value is String) {
    final normalized = value.startsWith('#') ? value.substring(1) : value;
    return int.parse(normalized, radix: 16);
  }
  throw const FormatException('Expected integer or hex string at "color".');
}

List<Object?> _readList(Object? value) {
  if (value is List<Object?>) {
    return value;
  }
  if (value is List) {
    return value.cast<Object?>();
  }
  throw const FormatException('Expected list.');
}

Map<String, Object?> _readMap(Object? value) {
  if (value is Map<String, Object?>) {
    return value;
  }
  if (value is Map) {
    return value.cast<String, Object?>();
  }
  throw const FormatException('Expected object.');
}
