import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import '../models/research_event.dart';

const canvasSize = Size(1400, 900);
const canvasPadding = 86.0;
const dotSafeRadius = 96.0;
const eventCollisionRadius = 92.0;

class ArtifactLayout {
  const ArtifactLayout({
    required this.artifact,
    required this.lines,
    required this.offset,
    required this.radius,
  });

  final SourceArtifact artifact;
  final List<String> lines;
  final Offset offset;
  final double radius;

  double get collisionRadius => radius + 26;
}

class EventLayout {
  const EventLayout({
    required this.event,
    required this.base,
    required this.display,
    required this.artifacts,
    required this.radius,
  });

  final ResearchEvent event;
  final Offset base;
  final Offset display;
  final List<ArtifactLayout> artifacts;
  final double radius;

  EventLayout copyWith({Offset? display}) {
    return EventLayout(
      event: event,
      base: base,
      display: display ?? this.display,
      artifacts: artifacts,
      radius: radius,
    );
  }
}

Map<String, Offset> generateBasePositions(
  List<ResearchEvent> events, {
  List<EventBridge> bridges = const [],
}) {
  final positions = <String, Offset>{};
  final components = _connectedComponents(events, bridges);

  for (var componentIndex = 0;
      componentIndex < components.length;
      componentIndex += 1) {
    final center = _clusterCenter(componentIndex);
    positions.addAll(_componentPositions(components[componentIndex], center));
  }

  return _relaxedGlobalComponentPositions(components, positions);
}

Offset? cameraTargetForEvents(
  Set<String> ids,
  Map<String, Offset> positions, {
  Size visibleWorldSize = canvasSize,
}) {
  final points = [
    for (final id in ids)
      if (positions[id] != null) positions[id]!,
  ];
  if (points.isEmpty) {
    return null;
  }
  final center =
      points.reduce((value, point) => value + point) / points.length.toDouble();
  return center -
      Offset(visibleWorldSize.width / 2, visibleWorldSize.height / 2);
}

List<List<ResearchEvent>> _connectedComponents(
  List<ResearchEvent> events,
  List<EventBridge> bridges,
) {
  final byId = {for (final event in events) event.id: event};
  final adjacency = {
    for (final event in events) event.id: <String>{},
  };
  for (final bridge in bridges) {
    if (!byId.containsKey(bridge.from) || !byId.containsKey(bridge.to)) {
      continue;
    }
    adjacency[bridge.from]!.add(bridge.to);
    adjacency[bridge.to]!.add(bridge.from);
  }

  final seen = <String>{};
  final components = <List<ResearchEvent>>[];
  for (final event in events) {
    if (!seen.add(event.id)) {
      continue;
    }
    final queue = <String>[event.id];
    final component = <ResearchEvent>[];
    for (var index = 0; index < queue.length; index += 1) {
      final id = queue[index];
      component.add(byId[id]!);
      for (final neighbor in adjacency[id]!) {
        if (seen.add(neighbor)) {
          queue.add(neighbor);
        }
      }
    }
    components.add(component);
  }
  return components;
}

Offset _clusterCenter(int index) {
  final origin = Offset(canvasSize.width / 2, canvasSize.height / 2 - 24);
  const stepX = 860.0;
  const stepY = 640.0;
  const slots = <Offset>[
    Offset.zero,
    Offset(-stepX, 0),
    Offset(stepX, 0),
    Offset(0, -stepY),
    Offset(0, stepY),
    Offset(-stepX, -stepY),
    Offset(stepX, -stepY),
    Offset(-stepX, stepY),
    Offset(stepX, stepY),
  ];
  if (index < slots.length) {
    return origin + slots[index];
  }

  final ring = ((index - slots.length) ~/ 8) + 2;
  final side = (index - slots.length) % 8;
  final angle = side * math.pi / 4;
  return origin +
      Offset(math.cos(angle) * stepX * ring, math.sin(angle) * stepY * ring);
}

Map<String, Offset> _componentPositions(
  List<ResearchEvent> events,
  Offset center,
) {
  final placed = <({Offset point, double radius})>[];
  final positions = <String, Offset>{};
  final footprintById = {
    for (final event in events) event.id: eventFootprintRadius(event),
  };
  final radius = math.max(260.0, math.sqrt(events.length) * 172);

  for (var index = 0; index < events.length; index += 1) {
    final event = events[index];
    final eventRadius = footprintById[event.id]!;
    final random =
        _SeededRandom(_hashString('event:${event.id}:${event.date}'));
    Offset? best;
    var bestScore = double.negativeInfinity;

    for (var attempt = 0; attempt < 320; attempt += 1) {
      final nearCenter = index == 0;
      final distance = nearCenter
          ? random.next() * 34
          : math.pow(random.next(), 0.62).toDouble() * radius;
      final angle = random.next() * math.pi * 2;
      final candidate = center +
          Offset(
            math.cos(angle) * distance,
            math.sin(angle) * distance * 0.74,
          );
      final minDistance = placed.isEmpty
          ? 999.0
          : placed
              .map((point) => (point.point - candidate).distance)
              .reduce(math.min);
      final recordPenalty =
          (candidate - Offset(canvasSize.width / 2, canvasSize.height - 74))
                      .distance <
                  170
              ? 90.0
              : 0.0;
      final nearestClearance = placed.isEmpty
          ? 999.0
          : placed
              .map(
                (point) =>
                    (point.point - candidate).distance -
                    point.radius -
                    eventRadius,
              )
              .reduce(math.min);
      final score = nearestClearance * 1.8 +
          minDistance * 0.25 -
          recordPenalty +
          random.next() * 18;

      if (score > bestScore) {
        bestScore = score;
        best = candidate;
      }
    }

    placed.add((point: best!, radius: eventRadius));
    positions[event.id] = best;
  }
  return _relaxedComponentPositions(events, positions, footprintById, center);
}

double eventFootprintRadius(ResearchEvent event) {
  final titleLines = wrapLines(event.title, 18);
  final titleHeight = titleLines.length * 15.0;
  final estimatedWidth = math.min(
    142.0,
    titleLines.map((line) => line.length).reduce(math.max) * 13.0 * 0.55,
  );
  final estimatedHeight = 42.0 + titleHeight + 24.0;
  return math.max(
    eventCollisionRadius,
    math.sqrt(
          math.pow(estimatedWidth * 0.5, 2).toDouble() +
              math.pow(estimatedHeight, 2).toDouble(),
        ) +
        12.0,
  );
}

Map<String, Offset> _relaxedComponentPositions(
  List<ResearchEvent> events,
  Map<String, Offset> positions,
  Map<String, double> radiusById,
  Offset center,
) {
  if (events.length < 2) {
    return positions;
  }

  final ids = events.map((event) => event.id).toList(growable: false);
  final next = Map<String, Offset>.of(positions);
  for (var iteration = 0; iteration < 120; iteration += 1) {
    var moved = false;
    for (var aIndex = 0; aIndex < ids.length; aIndex += 1) {
      for (var bIndex = aIndex + 1; bIndex < ids.length; bIndex += 1) {
        final aId = ids[aIndex];
        final bId = ids[bIndex];
        final a = next[aId]!;
        final b = next[bId]!;
        final minimum = radiusById[aId]! + radiusById[bId]! + 34.0;
        if ((b - a).distance >= minimum) {
          continue;
        }
        final resolved = _resolvedPair(a, b, minimum);
        next[aId] = resolved.$1;
        next[bId] = resolved.$2;
        moved = true;
      }
    }
    if (!moved) {
      break;
    }
  }

  final centroid = next.values.reduce((value, point) => value + point) /
      next.length.toDouble();
  final recenter = center - centroid;
  return {
    for (final entry in next.entries) entry.key: entry.value + recenter,
  };
}

Map<String, Offset> _relaxedGlobalComponentPositions(
  List<List<ResearchEvent>> components,
  Map<String, Offset> positions,
) {
  if (components.length < 2) {
    return positions;
  }

  final footprints = [
    for (var index = 0; index < components.length; index += 1)
      _componentFootprint(index, components[index], positions),
  ];
  final centers = {
    for (final footprint in footprints) footprint.index: footprint.center,
  };

  for (var iteration = 0; iteration < 160; iteration += 1) {
    var moved = false;
    for (var aIndex = 0; aIndex < footprints.length; aIndex += 1) {
      for (var bIndex = aIndex + 1; bIndex < footprints.length; bIndex += 1) {
        final a = footprints[aIndex];
        final b = footprints[bIndex];
        final centerA = centers[a.index]!;
        final centerB = centers[b.index]!;
        final minimum = a.radius + b.radius + 190.0;
        if ((centerB - centerA).distance >= minimum) {
          continue;
        }
        final resolved = _resolvedPair(centerA, centerB, minimum);
        centers[a.index] = resolved.$1;
        centers[b.index] = resolved.$2;
        moved = true;
      }
    }
    if (!moved) {
      break;
    }
  }

  final next = Map<String, Offset>.of(positions);
  for (final footprint in footprints) {
    final delta = centers[footprint.index]! - footprint.center;
    for (final id in footprint.ids) {
      next[id] = next[id]! + delta;
    }
  }
  return next;
}

({int index, List<String> ids, Offset center, double radius})
    _componentFootprint(
  int index,
  List<ResearchEvent> events,
  Map<String, Offset> positions,
) {
  final ids = events.map((event) => event.id).toList(growable: false);
  final center =
      ids.map((id) => positions[id]!).reduce((value, point) => value + point) /
          ids.length.toDouble();
  var radius = 0.0;
  for (final event in events) {
    radius = math.max(
      radius,
      (positions[event.id]! - center).distance + eventFootprintRadius(event),
    );
  }
  return (index: index, ids: ids, center: center, radius: radius);
}

Map<String, EventLayout> displayLayout({
  required List<ResearchEvent> events,
  required Map<String, Offset> basePositions,
  required String? activeId,
}) {
  final layouts = <String, EventLayout>{};
  for (final event in events) {
    final artifactMetrics = layoutArtifacts(event);
    layouts[event.id] = EventLayout(
      event: event,
      base: basePositions[event.id]!,
      display: basePositions[event.id]!,
      artifacts: artifactMetrics.artifacts,
      radius: artifactMetrics.radius,
    );
  }

  if (activeId == null) {
    return layouts;
  }

  final active = layouts[activeId];
  if (active == null) {
    return layouts;
  }

  final activeRadius = active.radius + 36;
  final affected = <String>{};
  for (final entry in layouts.entries.toList()) {
    if (entry.key == activeId) {
      continue;
    }
    final current = entry.value;
    final delta = current.display - active.display;
    final distance = delta.distance == 0 ? 1.0 : delta.distance;
    final minimum = activeRadius + dotSafeRadius;
    var next = current.display;

    if (distance < minimum) {
      final push = minimum - distance;
      next += Offset(delta.dx / distance, delta.dy / distance) * push;
      affected.add(entry.key);
    }

    layouts[entry.key] = current.copyWith(display: next);
  }

  final artifactObstacles = [
    for (final artifact in active.artifacts)
      _CollisionObstacle(
        center: active.display + artifact.offset,
        radius: artifact.collisionRadius + 18,
      ),
  ];
  for (var iteration = 0; iteration < 18; iteration += 1) {
    for (final entry in layouts.entries.toList()) {
      if (entry.key == activeId) {
        continue;
      }
      var current = entry.value;
      var next = current.display;
      for (final obstacle in artifactObstacles) {
        final minimum = eventCollisionRadius + obstacle.radius;
        if (!affected.contains(entry.key) &&
            (next - obstacle.center).distance >= minimum) {
          continue;
        }
        affected.add(entry.key);
        next = _pushedPoint(
          point: next,
          fixed: obstacle.center,
          minimum: minimum,
        );
      }
      layouts[entry.key] = current.copyWith(display: next);
    }

    final movable = layouts.keys.where((id) => id != activeId).toList();
    for (var aIndex = 0; aIndex < movable.length; aIndex += 1) {
      for (var bIndex = aIndex + 1; bIndex < movable.length; bIndex += 1) {
        final aId = movable[aIndex];
        final bId = movable[bIndex];
        if (!affected.contains(aId) && !affected.contains(bId)) {
          continue;
        }
        final a = layouts[aId]!;
        final b = layouts[bId]!;
        final minimum = eventCollisionRadius * 2 + 18;
        if ((b.display - a.display).distance >= minimum) {
          continue;
        }
        final resolved = _resolvedPair(
          a.display,
          b.display,
          minimum,
        );
        affected.add(aId);
        affected.add(bId);
        layouts[aId] = a.copyWith(display: resolved.$1);
        layouts[bId] = b.copyWith(display: resolved.$2);
      }
    }
  }

  return layouts;
}

class _CollisionObstacle {
  const _CollisionObstacle({required this.center, required this.radius});

  final Offset center;
  final double radius;
}

Offset _pushedPoint({
  required Offset point,
  required Offset fixed,
  required double minimum,
}) {
  final delta = point - fixed;
  final distance = delta.distance == 0 ? 1.0 : delta.distance;
  if (distance >= minimum) {
    return point;
  }
  return point +
      Offset(delta.dx / distance, delta.dy / distance) * (minimum - distance);
}

(Offset, Offset) _resolvedPair(Offset a, Offset b, double minimum) {
  final delta = b - a;
  final distance = delta.distance;
  if (distance >= minimum) {
    return (a, b);
  }
  final push = (minimum - distance) * 0.5;
  final direction = distance == 0
      ? const Offset(1, 0)
      : Offset(delta.dx / distance, delta.dy / distance);
  return (a - direction * push, b + direction * push);
}

({List<ArtifactLayout> artifacts, double radius}) layoutArtifacts(
    ResearchEvent event) {
  if (!event.canExpand) {
    return (artifacts: <ArtifactLayout>[], radius: 42);
  }

  final random = _SeededRandom(_hashString(event.id));
  final artifacts = event.artifacts.map(_artifactLayout).toList();
  final largest = artifacts.map((artifact) => artifact.radius).reduce(math.max);
  final ring = 150 + largest * 0.38;
  final rotation = -math.pi / 2 + (random.next() - 0.5) * 0.55;
  final mutable = artifacts.asMap().entries.map(
    (entry) {
      final angle = rotation +
          (entry.key * math.pi * 2) / artifacts.length +
          (random.next() - 0.5) * 0.18;
      final distance = ring + (random.next() - 0.5) * 18;
      return _MutableArtifactLayout(
        artifact: entry.value.artifact,
        lines: entry.value.lines,
        radius: entry.value.radius,
        x: math.cos(angle) * distance,
        y: math.sin(angle) * distance,
      );
    },
  ).toList();

  for (var iteration = 0; iteration < 80; iteration += 1) {
    for (final artifact in mutable) {
      final centerDistance =
          math.sqrt(artifact.x * artifact.x + artifact.y * artifact.y);
      final safeDistance = centerDistance == 0 ? 1.0 : centerDistance;
      final minimum = 82 + artifact.radius;
      if (safeDistance < minimum) {
        final push = (minimum - safeDistance) * 0.5;
        artifact.x += (artifact.x / safeDistance) * push;
        artifact.y += (artifact.y / safeDistance) * push;
      }
    }

    for (var aIndex = 0; aIndex < mutable.length; aIndex += 1) {
      for (var bIndex = aIndex + 1; bIndex < mutable.length; bIndex += 1) {
        final a = mutable[aIndex];
        final b = mutable[bIndex];
        final dx = b.x - a.x;
        final dy = b.y - a.y;
        final distance = math.sqrt(dx * dx + dy * dy);
        final safeDistance = distance == 0 ? 1.0 : distance;
        final minimum = a.collisionRadius + b.collisionRadius + 26;

        if (safeDistance < minimum) {
          final push = (minimum - safeDistance) * 0.5;
          final nx = dx / safeDistance;
          final ny = dy / safeDistance;
          a.x -= nx * push;
          a.y -= ny * push;
          b.x += nx * push;
          b.y += ny * push;
        }
      }
    }
  }

  var radius = 132.0;
  final finalArtifacts = mutable.map((artifact) {
    final offset = Offset(artifact.x, artifact.y);
    radius = math.max(radius, offset.distance + artifact.radius + 34);
    return ArtifactLayout(
      artifact: artifact.artifact,
      lines: artifact.lines,
      offset: offset,
      radius: artifact.radius,
    );
  }).toList();

  return (artifacts: finalArtifacts, radius: radius);
}

Path bridgePath(EventLayout from, EventLayout to) {
  final delta = to.display - from.display;
  final length = delta.distance == 0 ? 1.0 : delta.distance;
  final normal = Offset(-delta.dy / length, delta.dx / length);
  final control =
      from.display + delta * 0.5 + normal * math.min(72, length * 0.14);

  return Path()
    ..moveTo(from.display.dx, from.display.dy)
    ..quadraticBezierTo(control.dx, control.dy, to.display.dx, to.display.dy);
}

List<String> wrapLines(String content, int maxChars) {
  final lines = <String>[];
  var line = '';
  for (final word in content.split(' ')) {
    final trial = line.isEmpty ? word : '$line $word';
    if (trial.length > maxChars && line.isNotEmpty) {
      lines.add(line);
      line = word;
    } else {
      line = trial;
    }
  }
  lines.add(line);
  return lines;
}

ArtifactLayout _artifactLayout(SourceArtifact artifact) {
  final lines = wrapLines(artifact.text, 12);
  final radius = _textRadius(lines, 11, 16);
  return ArtifactLayout(
    artifact: artifact,
    lines: lines,
    offset: Offset.zero,
    radius: radius,
  );
}

double _textRadius(List<String> lines, double fontSize, double padding) {
  final longest = lines.map((line) => line.length).reduce(math.max);
  final estimatedWidth = longest * fontSize * 0.54;
  final estimatedHeight = lines.length * fontSize * 1.12;
  return (math.max(estimatedWidth / 2 + padding, estimatedHeight / 2 + padding))
      .ceilToDouble();
}

int _hashString(String value) {
  var hash = 2166136261;
  for (var index = 0; index < value.length; index += 1) {
    hash ^= value.codeUnitAt(index);
    hash = _imul32(hash, 16777619);
  }
  return hash & 0xffffffff;
}

int _imul32(int a, int b) {
  final result = (a & 0xffffffff) * (b & 0xffffffff);
  return result & 0xffffffff;
}

class _SeededRandom {
  _SeededRandom(this.state);

  int state;

  double next() {
    state = (_imul32(1664525, state) + 1013904223) & 0xffffffff;
    return state / 4294967296;
  }
}

class _MutableArtifactLayout {
  _MutableArtifactLayout({
    required this.artifact,
    required this.lines,
    required this.radius,
    required this.x,
    required this.y,
  });

  final SourceArtifact artifact;
  final List<String> lines;
  final double radius;
  double x;
  double y;

  double get collisionRadius => radius + 26;
}
