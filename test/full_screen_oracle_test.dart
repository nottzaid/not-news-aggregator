import 'package:ai_news_canvas/main.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

class _VisualOracleBinding extends AutomatedTestWidgetsFlutterBinding {
  @override
  bool get disableShadows => false;
}

void main() {
  _VisualOracleBinding();

  setUpAll(() async {
    final manrope = FontLoader('Manrope')
      ..addFont(rootBundle.load('assets/fonts/manrope/Manrope-Regular.ttf'));
    final jetBrainsMono = FontLoader('JetBrainsMono')
      ..addFont(
        rootBundle.load(
          'assets/fonts/jetbrainsmono/JetBrainsMono-Regular.ttf',
        ),
      );
    final materialIcons = FontLoader('MaterialIcons')
      ..addFont(
        rootBundle.load(
          'fonts/MaterialIcons-Regular.otf',
        ),
      );
    await Future.wait([
      manrope.load(),
      jetBrainsMono.load(),
      materialIcons.load(),
    ]);
  });

  testWidgets('records fixed desktop chrome over the closed canvas',
      (tester) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1280, 800);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    const oracle = Key('full-screen-closed');
    await tester.pumpWidget(buildCanvasFullScreenOracle(oracleKey: oracle));
    await tester.pump();
    await expectLater(
      find.byKey(oracle),
      matchesGoldenFile('goldens/full-screen-closed-1280x800.png'),
    );
  });

  testWidgets('records the same desktop frame without fixed chrome',
      (tester) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1280, 800);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    const oracle = Key('full-screen-base');
    await tester.pumpWidget(
      buildCanvasFullScreenOracle(oracleKey: oracle, showChrome: false),
    );
    await tester.pump();
    await expectLater(
      find.byKey(oracle),
      matchesGoldenFile('goldens/full-screen-base-1280x800.png'),
    );
  });

  testWidgets('records the busy record orb used during capture and research',
      (tester) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1280, 800);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    const oracle = Key('full-screen-record-busy');
    await tester.pumpWidget(
      buildCanvasFullScreenOracle(oracleKey: oracle, recordBusy: true),
    );
    await tester.pump();
    await expectLater(
      find.byKey(oracle),
      matchesGoldenFile('goldens/full-screen-record-busy-1280x800.png'),
    );
  });

  testWidgets('records expanded-event metadata over desktop chrome',
      (tester) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1280, 800);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    const oracle = Key('full-screen-active');
    await tester.pumpWidget(
      buildCanvasFullScreenOracle(oracleKey: oracle, activeId: 'spacex'),
    );
    await tester.pump();
    await expectLater(
      find.byKey(oracle),
      matchesGoldenFile('goldens/full-screen-active-1280x800.png'),
    );
  });

  testWidgets('records the same expanded frame without metadata',
      (tester) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1280, 800);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    const oracle = Key('full-screen-active-base');
    await tester.pumpWidget(
      buildCanvasFullScreenOracle(
        oracleKey: oracle,
        activeId: 'spacex',
        showMetadata: false,
      ),
    );
    await tester.pump();
    await expectLater(
      find.byKey(oracle),
      matchesGoldenFile('goldens/full-screen-active-base-1280x800.png'),
    );
  });

  testWidgets('records visible graph-unavailable status', (tester) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1280, 800);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    const oracle = Key('full-screen-status');
    await tester.pumpWidget(
      buildCanvasFullScreenOracle(
        oracleKey: oracle,
        statusMessage: 'Graph unavailable; no research is shown or writable.',
      ),
    );
    await tester.pump();
    await expectLater(
      find.byKey(oracle),
      matchesGoldenFile('goldens/full-screen-status-1280x800.png'),
    );
  });

  testWidgets('records open live Hermes activity over desktop canvas',
      (tester) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(1280, 800);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    const oracle = Key('full-screen-activity');
    await tester.pumpWidget(
      buildCanvasFullScreenOracle(
        oracleKey: oracle,
        activityMessages: const [
          'OpenCode research started.',
          'Searching official primary sources.',
          'Accepted finding: Rust language release',
        ],
        activityRunning: true,
        activityOpen: true,
      ),
    );
    await tester.pump(const Duration(milliseconds: 180));
    await expectLater(
      find.byKey(oracle),
      matchesGoldenFile('goldens/full-screen-activity-1280x800.png'),
    );
  });
}
