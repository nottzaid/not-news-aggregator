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
}

Future<void> expectGraphFrame(
  WidgetTester tester, {
  required Size size,
  required String golden,
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
          painter: buildClosedGraphOraclePainter(
            events: const [first, second],
            bridges: bridges,
            layouts: layouts,
          ),
        ),
      ),
    ),
  );
  await tester.pump();

  await expectLater(find.byKey(oracle), matchesGoldenFile(golden));
}
