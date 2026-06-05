import 'package:ai_news_canvas/canvas/canvas_layout.dart';
import 'package:ai_news_canvas/data/fixture_events.dart';
import 'package:ai_news_canvas/models/research_event.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  test('one-source related events do not get fake artifact graphs', () {
    final amazon = fixtureEvents.firstWhere((event) => event.id == 'amazon');

    final layout = layoutArtifacts(amazon);

    expect(amazon.canExpand, isFalse);
    expect(layout.artifacts, isEmpty);
    expect(layout.radius, 42);
  });

  test('single-artifact events open directly instead of expanding', () {
    const event = ResearchEvent(
      id: 'single-artifact',
      title: 'Single artifact',
      date: 'Jun 4, 2026',
      color: 0xff445566,
      summary: 'One source should not become a one-leaf graph.',
      sourceLabel: 'Official',
      artifacts: [
        SourceArtifact(
          text: 'Official post',
          source: 'Official',
          url: 'https://example.com/source',
        ),
      ],
    );

    final layout = layoutArtifacts(event);

    expect(event.canExpand, isFalse);
    expect(event.directUrl, 'https://example.com/source');
    expect(layout.artifacts, isEmpty);
  });

  test('base event placement is deterministic', () {
    final first = generateBasePositions(fixtureEvents, bridges: fixtureBridges);
    final second =
        generateBasePositions(fixtureEvents, bridges: fixtureBridges);

    expect(first, second);
  });

  test('unrelated graph components are placed in separate canvas regions', () {
    const events = [
      ResearchEvent(
        id: 'cosmos-a',
        title: 'Cosmos A',
        date: '2026-01-01',
        color: 0xff20aa44,
        summary: 'First Cosmos event.',
        sourceLabel: 'Source',
        artifacts: [],
      ),
      ResearchEvent(
        id: 'cosmos-b',
        title: 'Cosmos B',
        date: '2026-01-02',
        color: 0xff20aa44,
        summary: 'Second Cosmos event.',
        sourceLabel: 'Source',
        artifacts: [],
      ),
      ResearchEvent(
        id: 'kimi-a',
        title: 'Kimi A',
        date: '2026-02-01',
        color: 0xff6688ff,
        summary: 'First Kimi event.',
        sourceLabel: 'Source',
        artifacts: [],
      ),
      ResearchEvent(
        id: 'kimi-b',
        title: 'Kimi B',
        date: '2026-02-02',
        color: 0xff6688ff,
        summary: 'Second Kimi event.',
        sourceLabel: 'Source',
        artifacts: [],
      ),
    ];
    const bridges = [
      EventBridge(from: 'cosmos-a', to: 'cosmos-b', label: 'same topic'),
      EventBridge(from: 'kimi-a', to: 'kimi-b', label: 'same topic'),
    ];

    final positions = generateBasePositions(events, bridges: bridges);
    final cosmosCenter = (positions['cosmos-a']! + positions['cosmos-b']!) / 2;
    final kimiCenter = (positions['kimi-a']! + positions['kimi-b']!) / 2;

    expect((cosmosCenter - kimiCenter).distance, greaterThan(620));
    expect(
      (positions['cosmos-a']! - positions['cosmos-b']!).distance,
      lessThan(430),
    );
    expect(
      (positions['kimi-a']! - positions['kimi-b']!).distance,
      lessThan(430),
    );
  });

  test('camera target centers selected generated points', () {
    final target = cameraTargetForEvents(
      {'a', 'b'},
      {
        'a': const Offset(1600, 900),
        'b': const Offset(1800, 700),
        'ignored': const Offset(-3000, -2000),
      },
    );

    expect(target, const Offset(1000, 350));
    expect(cameraTargetForEvents({'missing'}, const {}), isNull);
  });

  test('connected clusters reserve space for long labels', () {
    final events = List.generate(
      9,
      (index) => ResearchEvent(
        id: 'dense-$index',
        title:
            'European AI sovereignty package component with long label $index',
        date: '2026-06-0${(index % 8) + 1}',
        color: 0xff76b900 + index,
        summary: 'Dense event $index.',
        sourceLabel: 'Source',
        artifacts: const [],
      ),
    );
    final bridges = [
      for (var index = 0; index < events.length - 1; index += 1)
        EventBridge(
          from: events[index].id,
          to: events[index + 1].id,
          label: 'same connected component',
        ),
    ];

    final positions = generateBasePositions(events, bridges: bridges);

    for (var aIndex = 0; aIndex < events.length; aIndex += 1) {
      for (var bIndex = aIndex + 1; bIndex < events.length; bIndex += 1) {
        final a = events[aIndex];
        final b = events[bIndex];
        final distance = (positions[a.id]! - positions[b.id]!).distance;
        final minimum = eventFootprintRadius(a) + eventFootprintRadius(b);

        expect(distance, greaterThanOrEqualTo(minimum));
      }
    }
  });

  test('disconnected dense clusters do not overlap across regions', () {
    final events = <ResearchEvent>[
      for (var cluster = 0; cluster < 4; cluster += 1)
        for (var index = 0; index < 7; index += 1)
          ResearchEvent(
            id: 'cluster-$cluster-event-$index',
            title:
                'Long research node cluster $cluster with neighboring topic label $index',
            date: '2026-06-0${(index % 8) + 1}',
            color: 0xff445566 + cluster * 20 + index,
            summary: 'Dense disconnected event $cluster/$index.',
            sourceLabel: 'Source',
            artifacts: const [],
          ),
    ];
    final bridges = <EventBridge>[
      for (var cluster = 0; cluster < 4; cluster += 1)
        for (var index = 0; index < 6; index += 1)
          EventBridge(
            from: 'cluster-$cluster-event-$index',
            to: 'cluster-$cluster-event-${index + 1}',
            label: 'same topic',
          ),
    ];

    final positions = generateBasePositions(events, bridges: bridges);

    for (var aIndex = 0; aIndex < events.length; aIndex += 1) {
      for (var bIndex = aIndex + 1; bIndex < events.length; bIndex += 1) {
        final a = events[aIndex];
        final b = events[bIndex];
        final distance = (positions[a.id]! - positions[b.id]!).distance;
        final minimum = eventFootprintRadius(a) + eventFootprintRadius(b);

        expect(distance, greaterThanOrEqualTo(minimum));
      }
    }
  });

  test('active expandable event pushes neighboring points away', () {
    final base = {
      for (final entry
          in generateBasePositions(fixtureEvents, bridges: fixtureBridges)
              .entries)
        entry.key: entry.value,
      'spacex': const Offset(650, 420),
      'amazon': const Offset(690, 420),
    };
    final inactive = displayLayout(
      events: fixtureEvents,
      basePositions: base,
      activeId: null,
    );
    final active = displayLayout(
      events: fixtureEvents,
      basePositions: base,
      activeId: 'spacex',
    );

    expect(inactive['spacex']!.display, active['spacex']!.display);
    expect(inactive['amazon']!.display, isNot(active['amazon']!.display));
    expect(
      (active['amazon']!.display - active['spacex']!.display).distance,
      greaterThan(
          (inactive['amazon']!.display - inactive['spacex']!.display).distance),
    );
  });

  test('active artifact nodes push nearby graph points away', () {
    const activeEvent = ResearchEvent(
      id: 'active',
      title: 'Active',
      date: 'Jun 4, 2026',
      color: 0xff76b900,
      summary: 'Active event.',
      sourceLabel: 'Source',
      artifacts: [
        SourceArtifact(text: 'One', source: 'Source', url: 'https://one.test'),
        SourceArtifact(text: 'Two', source: 'Source', url: 'https://two.test'),
      ],
    );
    const neighbor = ResearchEvent(
      id: 'neighbor',
      title: 'Neighbor label',
      date: 'Jun 4, 2026',
      color: 0xff445566,
      summary: 'Neighbor event.',
      sourceLabel: 'Source',
      artifacts: [],
    );
    final activeArtifacts = layoutArtifacts(activeEvent).artifacts;
    final obstacle = activeArtifacts.first;
    final base = {
      'active': const Offset(700, 450),
      'neighbor': const Offset(700, 450) + obstacle.offset + const Offset(8, 0),
    };

    final layout = displayLayout(
      events: const [activeEvent, neighbor],
      basePositions: base,
      activeId: 'active',
    );
    final obstacleCenter = layout['active']!.display + obstacle.offset;

    expect(
      (layout['neighbor']!.display - obstacleCenter).distance,
      greaterThan(eventCollisionRadius + obstacle.collisionRadius),
    );
  });

  test('non-active graph points also separate during expansion', () {
    const activeEvent = ResearchEvent(
      id: 'active',
      title: 'Active',
      date: 'Jun 4, 2026',
      color: 0xff76b900,
      summary: 'Active event.',
      sourceLabel: 'Source',
      artifacts: [
        SourceArtifact(text: 'One', source: 'Source', url: 'https://one.test'),
        SourceArtifact(text: 'Two', source: 'Source', url: 'https://two.test'),
      ],
    );
    const first = ResearchEvent(
      id: 'first',
      title: 'First nearby point',
      date: 'Jun 4, 2026',
      color: 0xff445566,
      summary: 'First event.',
      sourceLabel: 'Source',
      artifacts: [],
    );
    const second = ResearchEvent(
      id: 'second',
      title: 'Second nearby point',
      date: 'Jun 4, 2026',
      color: 0xff889944,
      summary: 'Second event.',
      sourceLabel: 'Source',
      artifacts: [],
    );
    final obstacle = layoutArtifacts(activeEvent).artifacts.first;
    final firstStart =
        const Offset(700, 450) + obstacle.offset + const Offset(8, 0);
    final base = {
      'active': const Offset(700, 450),
      'first': firstStart,
      'second': firstStart + const Offset(20, 8),
    };

    final layout = displayLayout(
      events: const [activeEvent, first, second],
      basePositions: base,
      activeId: 'active',
    );

    expect(
      (layout['first']!.display - layout['second']!.display).distance,
      greaterThan(eventCollisionRadius * 2),
    );
  });

  test('distant tight clusters stay still during unrelated expansion', () {
    const activeEvent = ResearchEvent(
      id: 'active',
      title: 'Active',
      date: 'Jun 4, 2026',
      color: 0xff76b900,
      summary: 'Active event.',
      sourceLabel: 'Source',
      artifacts: [
        SourceArtifact(text: 'One', source: 'Source', url: 'https://one.test'),
        SourceArtifact(text: 'Two', source: 'Source', url: 'https://two.test'),
      ],
    );
    const farA = ResearchEvent(
      id: 'far-a',
      title: 'Far A',
      date: 'Jun 4, 2026',
      color: 0xff445566,
      summary: 'Far event.',
      sourceLabel: 'Source',
      artifacts: [],
    );
    const farB = ResearchEvent(
      id: 'far-b',
      title: 'Far B',
      date: 'Jun 4, 2026',
      color: 0xff889944,
      summary: 'Far event.',
      sourceLabel: 'Source',
      artifacts: [],
    );
    const base = {
      'active': Offset(0, 0),
      'far-a': Offset(2200, 1400),
      'far-b': Offset(2220, 1408),
    };

    final layout = displayLayout(
      events: const [activeEvent, farA, farB],
      basePositions: base,
      activeId: 'active',
    );

    expect(layout['far-a']!.display, base['far-a']);
    expect(layout['far-b']!.display, base['far-b']);
  });
}
