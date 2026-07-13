import 'dart:ui' show Size;

import 'package:ai_news_canvas/main.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  testWidgets('records the canonical 320×180 background raster', (tester) async {
    tester.view.devicePixelRatio = 1;
    tester.view.physicalSize = const Size(320, 180);
    addTearDown(tester.view.resetDevicePixelRatio);
    addTearDown(tester.view.resetPhysicalSize);

    const oracle = Key('background-oracle');
    await tester.pumpWidget(
      const Directionality(
        textDirection: TextDirection.ltr,
        child: RepaintBoundary(
          key: oracle,
          child: CustomPaint(painter: CanvasBackgroundPainter()),
        ),
      ),
    );
    await tester.pump();

    await expectLater(
      find.byKey(oracle),
      matchesGoldenFile('goldens/background-320x180.png'),
    );
  });
}
