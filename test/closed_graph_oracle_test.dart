import 'package:ai_news_canvas/canvas/canvas_layout.dart';
import 'package:ai_news_canvas/main.dart';
import 'package:ai_news_canvas/models/research_event.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

const first = ResearchEvent(
  id: 'first',
  title: 'Orbital model accord',
  date: 'Jul 14, 2026',
  color: 0xffe8a44c,
  summary: 'First event.',
  sourceLabel: 'Source',
  artifacts: [],
);
const second = ResearchEvent(
  id: 'second',
  title: 'Compute treaty',
  date: 'Jul 15, 2026',
  color: 0xff4cc9d6,
  summary: 'Second event.',
  sourceLabel: 'Source',
  artifacts: [],
);
const firstPoint = Offset(550, 450);
const secondPoint = Offset(850, 450);
const layouts = {
  'first': EventLayout(
    event: first,
    base: firstPoint,
    display: firstPoint,
    artifacts: [],
    radius: 42,
  ),
  'second': EventLayout(
    event: second,
    base: secondPoint,
    display: secondPoint,
    artifacts: [],
    radius: 42,
  ),
};
const bridges = [
  EventBridge(from: 'first', to: 'second', label: 'causes'),
];
const artifactEvent = ResearchEvent(
  id: 'artifact-oracle',
  title: 'Artifact oracle',
  date: 'Jul 14, 2026',
  color: 0xffe8a44c,
  summary: 'Expanded evidence.',
  sourceLabel: 'Source',
  artifacts: [
    SourceArtifact(
      text: 'Official model release',
      source: 'Official',
      url: 'https://example.com/official',
    ),
    SourceArtifact(
      text: 'Independent report with a materially longer finding',
      source: 'Report',
      url: 'https://example.com/report',
    ),
    SourceArtifact(
      text: 'Concise synthesis',
      source: 'Summary',
      url: 'https://example.com/summary',
    ),
  ],
);
const displacedNeighbor = ResearchEvent(
  id: 'displaced-neighbor',
  title: 'Adjacent finding',
  date: 'Jul 15, 2026',
  color: 0xff4cc9d6,
  summary: 'Neighbor displaced by expanded evidence.',
  sourceLabel: 'Source',
  artifacts: [],
);

void main() {
  setUpAll(() async {
    final manrope = FontLoader('Manrope')
      ..addFont(rootBundle.load('assets/fonts/manrope/Manrope-Regular.ttf'));
    final jetBrainsMono = FontLoader('JetBrainsMono')
      ..addFont(
        rootBundle.load(
          'assets/fonts/jetbrainsmono/JetBrainsMono-Regular.ttf',
        ),
      );
    await Future.wait([manrope.load(), jetBrainsMono.load()]);
  });

  testWidgets('records the reference-size closed graph', (tester) async {
    await expectGraphFrame(
      tester,
      size: const Size(1400, 900),
      golden: 'goldens/closed-graph-1400x900.png',
    );
  });

  testWidgets('records the scaled closed graph', (tester) async {
    await expectGraphFrame(
      tester,
      size: const Size(480, 270),
      golden: 'goldens/closed-graph-480x270.png',
    );
  });

  testWidgets('records the fully expanded artifact graph', (tester) async {
    await expectArtifactFrame(
      tester,
      progress: 1,
      golden: 'goldens/artifact-graph-open-1400x900.png',
    );
  });

  testWidgets('records the artifact graph at half expansion', (tester) async {
    await expectArtifactFrame(
      tester,
      progress: 0.5,
      golden: 'goldens/artifact-graph-half-1400x900.png',
    );
  });

  testWidgets('records expanded neighbor displacement and bridge emphasis',
      (tester) async {
    const events = [artifactEvent, displacedNeighbor];
    const bridge = EventBridge(
      from: 'artifact-oracle',
      to: 'displaced-neighbor',
      label: 'informs',
    );
    final frameLayouts = displayLayout(
      events: events,
      basePositions: const {
        'artifact-oracle': Offset(700, 450),
        'displaced-neighbor': Offset(790, 450),
      },
      activeId: artifactEvent.id,
    );
    await expectGraphFrame(
      tester,
      size: const Size(1400, 900),
      golden: 'goldens/artifact-neighbor-open-1400x900.png',
      events: events,
      frameBridges: const [bridge],
      frameLayouts: frameLayouts,
      activeId: artifactEvent.id,
      bridgeActiveId: artifactEvent.id,
      expansionProgresses: const {'artifact-oracle': 1},
    );
  });

  testWidgets('records the temporal midpoint of activation and displacement',
      (tester) async {
    const events = [artifactEvent, displacedNeighbor];
    const bridge = EventBridge(
      from: 'artifact-oracle',
      to: 'displaced-neighbor',
      label: 'informs',
    );
    const base = {
      'artifact-oracle': Offset(700, 450),
      'displaced-neighbor': Offset(790, 450),
    };
    final closed = displayLayout(
      events: events,
      basePositions: base,
      activeId: null,
    );
    final open = displayLayout(
      events: events,
      basePositions: base,
      activeId: artifactEvent.id,
    );
    final progress = Curves.easeOutCubic.transform(0.5);
    final frameLayouts = {
      for (final event in events)
        event.id: open[event.id]!.copyWith(
          display: Offset.lerp(
            closed[event.id]!.display,
            open[event.id]!.display,
            progress,
          )!,
        ),
    };
    await expectGraphFrame(
      tester,
      size: const Size(1400, 900),
      golden: 'goldens/artifact-neighbor-midpoint-1400x900.png',
      events: events,
      frameBridges: const [bridge],
      frameLayouts: frameLayouts,
      activeId: artifactEvent.id,
      bridgeActiveId: artifactEvent.id,
      expansionProgresses: {artifactEvent.id: progress},
    );
  });
}

Future<void> expectArtifactFrame(
  WidgetTester tester, {
  required double progress,
  required String golden,
}) async {
  final metrics = layoutArtifacts(artifactEvent);
  const center = Offset(700, 450);
  await expectGraphFrame(
    tester,
    size: const Size(1400, 900),
    golden: golden,
    events: const [artifactEvent],
    frameBridges: const [],
    frameLayouts: {
      artifactEvent.id: EventLayout(
        event: artifactEvent,
        base: center,
        display: center,
        artifacts: metrics.artifacts,
        radius: metrics.radius,
      ),
    },
    activeId: artifactEvent.id,
    bridgeActiveId: artifactEvent.id,
    expansionProgresses: {artifactEvent.id: progress},
  );
}

Future<void> expectGraphFrame(
  WidgetTester tester, {
  required Size size,
  required String golden,
  List<ResearchEvent> events = const [first, second],
  List<EventBridge> frameBridges = bridges,
  Map<String, EventLayout> frameLayouts = layouts,
  String? activeId,
  String? bridgeActiveId,
  Map<String, double> expansionProgresses = const {},
}) async {
  tester.view.devicePixelRatio = 1;
  tester.view.physicalSize = size;
  addTearDown(tester.view.resetDevicePixelRatio);
  addTearDown(tester.view.resetPhysicalSize);

  final oracle = Key(golden);
  await tester.pumpWidget(
    Directionality(
      textDirection: TextDirection.ltr,
      child: RepaintBoundary(
        key: oracle,
        child: CustomPaint(
          painter: buildCanvasGraphOraclePainter(
            events: events,
            bridges: frameBridges,
            layouts: frameLayouts,
            activeId: activeId,
            bridgeActiveId: bridgeActiveId,
            expansionProgresses: expansionProgresses,
          ),
        ),
      ),
    ),
  );
  await tester.pump();

  await expectLater(find.byKey(oracle), matchesGoldenFile(golden));
}
