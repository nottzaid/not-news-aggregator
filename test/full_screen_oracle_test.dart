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
}
