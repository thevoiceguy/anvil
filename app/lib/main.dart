import 'package:flutter/material.dart';

import 'src/app.dart';
import 'src/phone/rust_phone.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  runApp(AnvilApp(phone: RustPhone()));
}
