// Kitchen's Cook, Plan and Shopping tabs.
//
// Cook is one step at a time, large enough to read from across the counter,
// with the step's timers and the next step's first one; ← and → move, and
// Read aloud speaks each step in the Books reader's voice. Plan is the week
// as lunch and dinner. Shopping is the list by aisle, with the phone page
// and the shop logged as an expense in Finances.

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../design/tokens.dart';
import '../../src/rust/api/kitchen.dart';
import '../feeds/feeds_controller.dart' show FeedsVoice;
import '../transfer/qr_view.dart' show QrView;
import 'kitchen_controller.dart';
import 'kitchen_page.dart';

// -------------------------------------------------------------------- cook --

class CookView extends StatefulWidget {
  const CookView({super.key, required this.c, required this.st});

  final KitchenController c;
  final KitchenState st;

  @override
  State<CookView> createState() => _CookViewState();
}

class _CookViewState extends State<CookView> {
  final FocusNode _focus = FocusNode();
  final FeedsVoice _voice = FeedsVoice();
  bool _aloud = false;

  @override
  void initState() {
    super.initState();
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (mounted) _focus.requestFocus();
    });
  }

  @override
  void didUpdateWidget(CookView old) {
    super.didUpdateWidget(old);
    if (_aloud && widget.st.cookStep != old.st.cookStep) _speak();
  }

  @override
  void dispose() {
    _voice.dispose();
    _focus.dispose();
    super.dispose();
  }

  void _speak() {
    final r = widget.st.open;
    final n = widget.st.cookStep;
    if (r == null || n >= r.steps.length) return;
    _voice.read('Step ${n + 1}', r.steps[n].text);
  }

  KeyEventResult _key(FocusNode _, KeyEvent e) {
    if (e is! KeyDownEvent) return KeyEventResult.ignored;
    final r = widget.st.open;
    if (r == null) return KeyEventResult.ignored;
    final last = widget.st.cookStep >= r.steps.length - 1;
    if (e.logicalKey == LogicalKeyboardKey.arrowRight ||
        e.logicalKey == LogicalKeyboardKey.pageDown) {
      if (!last) widget.c.send(const KitchenCmd.cookStep(delta: 1));
      return KeyEventResult.handled;
    }
    if (e.logicalKey == LogicalKeyboardKey.arrowLeft ||
        e.logicalKey == LogicalKeyboardKey.pageUp) {
      widget.c.send(const KitchenCmd.cookStep(delta: -1));
      return KeyEventResult.handled;
    }
    return KeyEventResult.ignored;
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.c;
    final st = widget.st;
    final r = st.open;
    if (r == null || !st.cooking || r.steps.isEmpty) {
      return Padding(
        padding: const EdgeInsets.all(24),
        child: Column(
          children: [
            const Quiet(
              icon: Icons.soup_kitchen_outlined,
              title: 'Nothing on the stove',
              body:
                  'Open a recipe and press Start cooking: one step at a time, with its timers, and the screen kept on.',
            ),
            const SizedBox(height: 14),
            if (r != null && r.steps.isNotEmpty)
              FilledButton.icon(
                style: FilledButton.styleFrom(backgroundColor: kKitchen),
                onPressed: () => c.send(const KitchenCmd.startCook()),
                icon: const Icon(Icons.play_arrow),
                label: Text('Start cooking ${r.title}'),
              ),
          ],
        ),
      );
    }
    final n = st.cookStep.clamp(0, r.steps.length - 1).toInt();
    final step = r.steps[n];
    final last = n == r.steps.length - 1;
    final next = !last && r.steps[n + 1].timers.isNotEmpty ? r.steps[n + 1] : null;
    return Focus(
      focusNode: _focus,
      onKeyEvent: _key,
      child: GestureDetector(
        behavior: HitTestBehavior.translucent,
        onTap: _focus.requestFocus,
        child: ListView(
          padding: const EdgeInsets.fromLTRB(22, 14, 22, 40),
          children: [
            Row(
              children: [
                OutlinedButton.icon(
                  style: OutlinedButton.styleFrom(
                      visualDensity: VisualDensity.compact),
                  onPressed: () => c.send(const KitchenCmd.leaveCook()),
                  icon: const Icon(Icons.close, size: 15),
                  label: const Text('Leave cooking'),
                ),
                Expanded(
                  child: Text('${r.title} · the screen stays on',
                      textAlign: TextAlign.center,
                      overflow: TextOverflow.ellipsis,
                      style: TextStyle(fontSize: 12.5, color: t.nInk2)),
                ),
                Tooltip(
                  message: _aloud ? 'Stop reading steps' : 'Read each step aloud',
                  child: IconButton(
                    onPressed: () {
                      setState(() => _aloud = !_aloud);
                      if (_aloud) {
                        _speak();
                      } else {
                        _voice.stop();
                      }
                    },
                    icon: Icon(
                        _aloud ? Icons.volume_up : Icons.volume_up_outlined,
                        color: _aloud ? kKitchen : t.nInk3),
                  ),
                ),
                Text('Press → for the next step',
                    style: TextStyle(fontSize: 12, color: t.nInk3)),
              ],
            ),
            const SizedBox(height: 26),
            Center(
              child: ConstrainedBox(
                constraints: const BoxConstraints(maxWidth: 780),
                child: Column(
                  children: [
                    Text('Step ${n + 1} of ${r.steps.length}',
                        style: const TextStyle(
                            fontSize: 13,
                            fontWeight: FontWeight.w700,
                            color: kKitchen)),
                    const SizedBox(height: 12),
                    Text(step.text,
                        textAlign: TextAlign.center,
                        style: TextStyle(
                            fontFamily: kSerif,
                            fontSize: step.text.length > 160 ? 26 : 34,
                            height: 1.3,
                            fontWeight: FontWeight.w700,
                            color: t.nInk)),
                    if (step.uses.isNotEmpty) ...[
                      const SizedBox(height: 18),
                      Wrap(
                        alignment: WrapAlignment.center,
                        spacing: 8,
                        runSpacing: 8,
                        children: [
                          for (final u in step.uses)
                            Container(
                              padding: const EdgeInsets.symmetric(
                                  horizontal: 10, vertical: 5),
                              decoration: BoxDecoration(
                                color: t.nChip,
                                borderRadius: BorderRadius.circular(99),
                                border: Border.all(color: t.nHair),
                              ),
                              child: Text(u,
                                  style: TextStyle(
                                      fontSize: 12.5,
                                      fontWeight: FontWeight.w600,
                                      color: t.nInk2)),
                            ),
                        ],
                      ),
                    ],
                    const SizedBox(height: 26),
                    Wrap(
                      alignment: WrapAlignment.center,
                      spacing: 12,
                      runSpacing: 12,
                      children: [
                        for (var i = 0; i < step.timers.length; i++)
                          _TimerCard(
                            c: c,
                            t: c.timerFor(r.id, n, i, step.timers[i]),
                            sub: 'of ${countdown(step.timers[i].secs)}',
                            lead: true,
                          ),
                        if (next != null)
                          _TimerCard(
                            c: c,
                            t: c.timerFor(r.id, n + 1, 0, next.timers[0]),
                            sub: 'starts at step ${n + 2}',
                            lead: false,
                          ),
                      ],
                    ),
                    const SizedBox(height: 22),
                    Row(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        for (var i = 0; i < r.steps.length; i++)
                          Container(
                            width: i == n ? 26 : 18,
                            height: 5,
                            margin: const EdgeInsets.symmetric(horizontal: 3),
                            decoration: BoxDecoration(
                              color: i < n
                                  ? kKitchen.withValues(alpha: 0.45)
                                  : i == n
                                      ? kKitchen
                                      : t.nHair,
                              borderRadius: BorderRadius.circular(3),
                            ),
                          ),
                      ],
                    ),
                    const SizedBox(height: 18),
                    Row(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        OutlinedButton.icon(
                          onPressed: n == 0
                              ? null
                              : () => c.send(
                                  const KitchenCmd.cookStep(delta: -1)),
                          icon: const Icon(Icons.chevron_left),
                          label: const Text('Back'),
                        ),
                        const SizedBox(width: 10),
                        FilledButton.icon(
                          style: FilledButton.styleFrom(
                              backgroundColor: kKitchen,
                              padding: const EdgeInsets.symmetric(
                                  horizontal: 22, vertical: 14)),
                          onPressed: () => c.send(last
                              ? const KitchenCmd.finishCook()
                              : const KitchenCmd.cookStep(delta: 1)),
                          icon: Icon(last ? Icons.check : Icons.chevron_right),
                          label: Text(last ? 'Done — dish up' : 'Next step'),
                        ),
                      ],
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}

class _TimerCard extends StatelessWidget {
  const _TimerCard({
    required this.c,
    required this.t,
    required this.sub,
    required this.lead,
  });

  final KitchenController c;
  final CookTimer t;
  final String sub;

  /// This step's own timer, drawn a touch stronger than the next step's.
  final bool lead;

  @override
  Widget build(BuildContext context) {
    final k = context.tokens;
    final live = t.running || (t.left < t.total && !t.done);
    return Container(
      width: 230,
      padding: const EdgeInsets.fromLTRB(12, 10, 10, 10),
      decoration: BoxDecoration(
        color: k.nCard,
        borderRadius: BorderRadius.circular(14),
        border: Border.all(
            color: t.done
                ? Tokens.error
                : live || lead
                    ? kKitchen.withValues(alpha: live ? 0.9 : 0.4)
                    : k.nHair,
            width: live || t.done ? 2 : 1),
      ),
      child: Row(
        children: [
          SizedBox(
            width: 46,
            height: 46,
            child: Stack(
              alignment: Alignment.center,
              children: [
                CircularProgressIndicator(
                  value: t.fraction,
                  strokeWidth: 4,
                  color: t.done ? Tokens.error : kKitchen,
                  backgroundColor: k.nHair,
                ),
                Text(countdown(t.left),
                    style: TextStyle(
                        fontSize: t.left >= 3600 ? 9 : 11.5,
                        fontWeight: FontWeight.w800,
                        color: k.nInk)),
              ],
            ),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(t.done ? '${t.label} — done' : t.label,
                    style: TextStyle(
                        fontSize: 13,
                        fontWeight: FontWeight.w700,
                        color: t.done ? Tokens.error : k.nInk)),
                Text(sub, style: TextStyle(fontSize: 11.5, color: k.nInk3)),
              ],
            ),
          ),
          IconButton(
            tooltip: t.done
                ? 'Again'
                : t.running
                    ? 'Pause'
                    : 'Start',
            onPressed: () => c.toggle(t),
            icon: Icon(
                t.done
                    ? Icons.replay
                    : t.running
                        ? Icons.pause
                        : Icons.play_arrow,
                size: 19),
          ),
        ],
      ),
    );
  }
}

// -------------------------------------------------------------------- plan --

class PlanView extends StatelessWidget {
  const PlanView({super.key, required this.c, required this.st});

  final KitchenController c;
  final KitchenState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final w = st.week;
    return ListView(
      padding: const EdgeInsets.fromLTRB(22, 16, 22, 40),
      children: [
        Row(
          children: [
            IconButton(
              tooltip: 'The week before',
              onPressed: () => c.send(const KitchenCmd.shiftWeek(delta: -1)),
              icon: const Icon(Icons.chevron_left),
            ),
            Text(w.title,
                style: TextStyle(
                    fontSize: 18, fontWeight: FontWeight.w800, color: t.nInk)),
            IconButton(
              tooltip: 'The week after',
              onPressed: () => c.send(const KitchenCmd.shiftWeek(delta: 1)),
              icon: const Icon(Icons.chevron_right),
            ),
            const SizedBox(width: 4),
            Flexible(
              child: Text(
                  '${w.range} · ${plural(w.planned, 'meal')} planned',
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 12.5, color: t.nInk3)),
            ),
            const Spacer(),
            OutlinedButton.icon(
              onPressed: () => c.send(const KitchenCmd.fillGaps()),
              icon: const Icon(Icons.auto_awesome_outlined, size: 16),
              label: const Text('Fill the gaps'),
            ),
            const SizedBox(width: 8),
            FilledButton.icon(
              style: FilledButton.styleFrom(backgroundColor: kKitchen),
              onPressed: () => c.send(const KitchenCmd.makeList()),
              icon: const Icon(Icons.checklist, size: 17),
              label: const Text('Make the shopping list'),
            ),
          ],
        ),
        const SizedBox(height: 14),
        LayoutBuilder(builder: (context, box) {
          const gap = 10.0;
          final col = ((box.maxWidth - gap * 6) / 7).clamp(140.0, 400.0).toDouble();
          final row = Row(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              for (var i = 0; i < w.days.length; i++) ...[
                if (i > 0) const SizedBox(width: gap),
                SizedBox(width: col, child: _DayColumn(c: c, st: st, d: w.days[i])),
              ],
            ],
          );
          return col * 7 + gap * 6 > box.maxWidth
              ? SingleChildScrollView(scrollDirection: Axis.horizontal, child: row)
              : row;
        }),
        if (st.total == 0) ...[
          const SizedBox(height: 16),
          Text('Save a few recipes first — Fill the gaps plans from them.',
              style: TextStyle(fontSize: 12.5, color: t.nInk3)),
        ],
      ],
    );
  }
}

class _DayColumn extends StatelessWidget {
  const _DayColumn({required this.c, required this.st, required this.d});

  final KitchenController c;
  final KitchenState st;
  final PlanDay d;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Opacity(
      opacity: d.past ? 0.6 : 1,
      child: Container(
        padding: const EdgeInsets.all(8),
        decoration: BoxDecoration(
          color: t.nChip,
          borderRadius: BorderRadius.circular(12),
          border: Border.all(
              color: d.today ? kKitchen : Colors.transparent, width: 1.5),
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Text('${d.dow} ${d.n}',
                style: TextStyle(
                    fontSize: 13, fontWeight: FontWeight.w800, color: t.nInk)),
            const SizedBox(height: 6),
            for (final (meal, slot) in [('lunch', d.lunch), ('dinner', d.dinner)]) ...[
              Text(meal == 'lunch' ? 'Lunch' : 'Dinner',
                  style: TextStyle(fontSize: 11, color: t.nInk3)),
              const SizedBox(height: 4),
              _Slot(c: c, st: st, day: d.day, meal: meal, s: slot),
              const SizedBox(height: 10),
            ],
          ],
        ),
      ),
    );
  }
}

class _Slot extends StatelessWidget {
  const _Slot({
    required this.c,
    required this.st,
    required this.day,
    required this.meal,
    required this.s,
  });

  final KitchenController c;
  final KitchenState st;
  final String day;
  final String meal;
  final PlanSlot s;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final empty = s.recipeId == 0 && s.label.isEmpty;
    final name = s.recipeId != 0 ? s.title : s.label;
    return InkWell(
      borderRadius: BorderRadius.circular(10),
      onTap: () => showSlot(context, c, st, day, meal, s),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          AspectRatio(
            aspectRatio: 4 / 3,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(10),
              child: empty
                  ? DecoratedBox(
                      decoration: BoxDecoration(
                        borderRadius: BorderRadius.circular(10),
                        border: Border.all(color: t.nInk3.withValues(alpha: 0.4)),
                      ),
                      child: Center(
                        child: Text('+ Add',
                            style: TextStyle(fontSize: 12, color: t.nInk3)),
                      ),
                    )
                  : s.recipeId != 0
                      ? Art(path: s.image, seed: s.recipeId)
                      : ColoredBox(color: t.nHair),
            ),
          ),
          if (!empty) ...[
            const SizedBox(height: 4),
            Text(name,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12, color: t.nInk)),
          ],
        ],
      ),
    );
  }
}

/// Pick a recipe for a meal, write something instead, or clear it.
Future<void> showSlot(BuildContext context, KitchenController c,
    KitchenState st, String day, String meal, PlanSlot s) {
  final label = TextEditingController(text: s.label);
  final filter = ValueNotifier('');
  Future<void> put(int recipe, String text) async {
    await c.send(KitchenCmd.setMeal(
        day: day, meal: meal, recipeId: recipe, label: text));
  }

  return showDialog<void>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text('${meal == 'lunch' ? 'Lunch' : 'Dinner'}, ${day.substring(5)}'),
      content: SizedBox(
        width: 420,
        height: 420,
        child: Column(
          children: [
            TextField(
              autofocus: true,
              decoration: const InputDecoration(
                  prefixIcon: Icon(Icons.search), hintText: 'Find a recipe'),
              onChanged: (v) => filter.value = v.toLowerCase(),
            ),
            const SizedBox(height: 8),
            Expanded(
              child: ValueListenableBuilder<String>(
                valueListenable: filter,
                builder: (ctx, f, _) {
                  final picks = [
                    for (final p in st.picks)
                      if (f.isEmpty || p.title.toLowerCase().contains(f)) p,
                  ];
                  if (picks.isEmpty) {
                    return const Center(child: Text('No recipe by that name'));
                  }
                  return ListView(
                    children: [
                      for (final p in picks)
                        ListTile(
                          dense: true,
                          selected: p.id == s.recipeId,
                          title: Text(p.title),
                          onTap: () async {
                            Navigator.pop(ctx);
                            await put(p.id, '');
                          },
                        ),
                    ],
                  );
                },
              ),
            ),
            const Divider(),
            TextField(
              controller: label,
              decoration: const InputDecoration(
                  hintText: 'Or write it: Out, Leftovers, Soup…'),
              onSubmitted: (v) async {
                Navigator.pop(ctx);
                await put(0, v);
              },
            ),
          ],
        ),
      ),
      actions: [
        if (s.recipeId != 0 || s.label.isNotEmpty)
          TextButton(
            onPressed: () async {
              Navigator.pop(ctx);
              await put(0, '');
            },
            child: const Text('Clear'),
          ),
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
          style: FilledButton.styleFrom(backgroundColor: kKitchen),
          onPressed: () async {
            Navigator.pop(ctx);
            if (label.text.trim().isNotEmpty) await put(0, label.text);
          },
          child: const Text('Use the text'),
        ),
      ],
    ),
  );
}

/// "Plan it" from a recipe: the week comes up, and a free slot is picked.
Future<void> planIt(BuildContext context, KitchenController c, int recipe) async {
  final st = await c.send(const KitchenCmd.setTab(tab: 'plan'));
  if (st == null || !context.mounted) return;
  final picked = await showDialog<(String, String)>(
    context: context,
    builder: (ctx) => SimpleDialog(
      title: Text('Plan it — ${st.week.title.toLowerCase()}'),
      children: [
        for (final d in st.week.days)
          if (!d.past)
            for (final (meal, slot) in [('lunch', d.lunch), ('dinner', d.dinner)])
              SimpleDialogOption(
                onPressed: () => Navigator.pop(ctx, (d.day, meal)),
                child: Text(
                    '${d.dow} ${d.n} · ${meal == 'lunch' ? 'Lunch' : 'Dinner'}'
                    '${slot.recipeId != 0 ? ' — replaces ${slot.title}' : slot.label.isNotEmpty ? ' — replaces ${slot.label}' : ''}'),
              ),
      ],
    ),
  );
  if (picked == null) return;
  await c.send(KitchenCmd.setMeal(
      day: picked.$1, meal: picked.$2, recipeId: recipe, label: ''));
}

// -------------------------------------------------------------------- shop --

class ShopList extends StatefulWidget {
  const ShopList({super.key, required this.c, required this.st});

  final KitchenController c;
  final KitchenState st;

  @override
  State<ShopList> createState() => _ShopListState();
}

class _ShopListState extends State<ShopList> {
  final TextEditingController _add = TextEditingController();

  @override
  void dispose() {
    _add.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.c;
    final s = widget.st.shop;
    final sub = [
      if (s.fromMeals > 0) 'from ${plural(s.fromMeals, 'planned meal')}',
      '${s.inBasket} of ${s.total} in the basket',
    ].join(' · ');
    return ListView(
      padding: const EdgeInsets.fromLTRB(22, 16, 22, 40),
      children: [
        Row(
          children: [
            Text('Shopping list',
                style: TextStyle(
                    fontSize: 18, fontWeight: FontWeight.w800, color: t.nInk)),
            const SizedBox(width: 10),
            Flexible(
              child: Text(sub,
                  overflow: TextOverflow.ellipsis,
                  style: TextStyle(fontSize: 12.5, color: t.nInk3)),
            ),
            const Spacer(),
            if (s.inBasket > 0) ...[
              TextButton.icon(
                onPressed: () => c.send(const KitchenCmd.clearBasket()),
                icon: const Icon(Icons.kitchen_outlined, size: 16),
                label: const Text('Put the basket away'),
              ),
              const SizedBox(width: 6),
            ],
            OutlinedButton.icon(
              onPressed: s.total == 0 ? null : () => showPhone(context, c),
              icon: const Icon(Icons.qr_code_2, size: 16),
              label: const Text('Open on phone'),
            ),
            const SizedBox(width: 8),
            OutlinedButton.icon(
              onPressed: widget.st.accounts.isEmpty
                  ? null
                  : () => showLog(context, c, widget.st),
              icon: const Icon(Icons.account_balance_wallet_outlined, size: 16),
              label: const Text('Log in Finances'),
            ),
          ],
        ),
        const SizedBox(height: 12),
        Container(
          decoration: cardDeco(context),
          padding: const EdgeInsets.fromLTRB(12, 2, 6, 2),
          child: Row(
            children: [
              Icon(Icons.add, size: 18, color: t.nInk3),
              const SizedBox(width: 8),
              Expanded(
                child: TextField(
                  controller: _add,
                  decoration: InputDecoration(
                    border: InputBorder.none,
                    hintText: 'Add to the list — “2 lemons”, “500 g rice”',
                    hintStyle: TextStyle(fontSize: 13, color: t.nInk3),
                  ),
                  onSubmitted: (v) async {
                    if (v.trim().isEmpty) return;
                    await c.send(KitchenCmd.addItem(text: v));
                    _add.clear();
                  },
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 14),
        if (s.groups.isEmpty)
          const Quiet(
            icon: Icons.shopping_basket_outlined,
            title: 'The list is empty',
            body:
                'Plan the week and press Make the shopping list, add what a recipe is missing, or type things in above.',
          )
        else
          Grid(
            min: 260,
            children: [
              for (final g in s.groups) _Aisle(c: c, g: g),
            ],
          ),
      ],
    );
  }
}

class _Aisle extends StatelessWidget {
  const _Aisle({required this.c, required this.g});

  final KitchenController c;
  final ShopGroup g;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: cardDeco(context),
      padding: const EdgeInsets.fromLTRB(12, 10, 6, 8),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Text(g.aisle,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const Spacer(),
              Text('${g.items.length}',
                  style: TextStyle(fontSize: 12, color: t.nInk3)),
              const SizedBox(width: 8),
            ],
          ),
          const SizedBox(height: 4),
          for (final i in g.items) ...[
            Divider(height: 1, color: t.nHair),
            Row(
              children: [
                Checkbox(
                  value: i.done,
                  activeColor: kKitchen,
                  visualDensity: VisualDensity.compact,
                  onChanged: (_) => c.send(KitchenCmd.toggleItem(id: i.id)),
                ),
                Expanded(
                  child: Text(i.name,
                      style: TextStyle(
                          fontSize: 13.5,
                          color: i.done ? t.nInk3 : t.nInk,
                          decoration:
                              i.done ? TextDecoration.lineThrough : null)),
                ),
                Text(i.amount,
                    style: TextStyle(fontSize: 12, color: t.nInk3)),
                IconButton(
                  tooltip: 'Take off the list',
                  iconSize: 15,
                  visualDensity: VisualDensity.compact,
                  onPressed: () => c.send(KitchenCmd.removeItem(id: i.id)),
                  icon: Icon(Icons.close, color: t.nInk3),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }
}

/// The list's page on the local network, as a QR code for the phone.
Future<void> showPhone(BuildContext context, KitchenController c) async {
  try {
    final link = await kitchenPhone();
    if (!context.mounted) return;
    final qr = link.qr;
    await showDialog<void>(
      context: context,
      builder: (ctx) => AlertDialog(
        title: const Text('Open on phone'),
        content: SizedBox(
          width: 320,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (qr != null)
                Container(
                  width: 240,
                  height: 240,
                  color: Colors.white,
                  child: QrView(code: qr),
                ),
              const SizedBox(height: 12),
              SelectableText(link.url,
                  textAlign: TextAlign.center,
                  style: const TextStyle(fontSize: 12)),
              const SizedBox(height: 8),
              const Text(
                'Scan it on the same wifi. The page shows the list as it is when it opens; a new code ends the old one.',
                textAlign: TextAlign.center,
                style: TextStyle(fontSize: 12),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Clipboard.setData(ClipboardData(text: link.url)),
            child: const Text('Copy link'),
          ),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: kKitchen),
            onPressed: () => Navigator.pop(ctx),
            child: const Text('Done'),
          ),
        ],
      ),
    );
  } catch (e) {
    c.say(plainError(e));
  }
}

/// The shop as an expense: which account, how much.
Future<void> showLog(
    BuildContext context, KitchenController c, KitchenState st) {
  var account = st.accounts.first.id;
  final amount = TextEditingController();
  String? error;
  return showDialog<void>(
    context: context,
    builder: (ctx) => StatefulBuilder(
      builder: (ctx, set) => AlertDialog(
        title: const Text('Log the shop in Finances'),
        content: SizedBox(
          width: 360,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              DropdownButtonFormField<int>(
                initialValue: account,
                decoration: const InputDecoration(labelText: 'Paid from'),
                items: [
                  for (final a in st.accounts)
                    DropdownMenuItem(
                        value: a.id, child: Text('${a.name} · ${a.currency}')),
                ],
                onChanged: (v) => set(() => account = v ?? account),
              ),
              TextField(
                controller: amount,
                autofocus: true,
                keyboardType:
                    const TextInputType.numberWithOptions(decimal: true),
                decoration: const InputDecoration(labelText: 'Amount'),
              ),
              if (error != null) ...[
                const SizedBox(height: 8),
                Text(error!,
                    style:
                        const TextStyle(color: Tokens.error, fontSize: 12.5)),
              ],
              const SizedBox(height: 8),
              const Text('Posted as “Food shop” today, under Groceries when there is such a category.',
                  style: TextStyle(fontSize: 12)),
            ],
          ),
        ),
        actions: [
          TextButton(
              onPressed: () => Navigator.pop(ctx),
              child: const Text('Cancel')),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: kKitchen),
            onPressed: () async {
              final err = await c.attempt(
                  KitchenCmd.logShop(accountId: account, amount: amount.text));
              if (!ctx.mounted) return;
              if (err == null) {
                Navigator.pop(ctx);
              } else {
                set(() => error = err);
              }
            },
            child: const Text('Log it'),
          ),
        ],
      ),
    ),
  );
}
