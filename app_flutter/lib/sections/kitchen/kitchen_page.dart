// The Kitchen section — docs/NewSections/kitchen-deck.html.
//
// Five tabs over one snapshot: Recipes (save from a link, filters, the
// cards), Recipe (servings that scale every quantity, what is in stock, the
// method with its timers), Cook, Plan and Shopping — the last three in
// kitchen_cook.dart. Every surface is drawn from the tokens and the skin; the
// one liberty is the serif for titles and steps, as in the deck.

import 'package:flutter/material.dart';

import '../../design/first_load.dart';
import '../../design/skin.dart';
import '../../design/tokens.dart';
import '../../shell/section_tabs.dart';
import '../../src/rust/api/dialog.dart';
import '../../src/rust/api/kitchen.dart';
import 'kitchen_controller.dart';
import 'kitchen_cook.dart';
import '../../design/decode.dart';

const String kSerif = 'serif';

class KitchenPage extends StatefulWidget {
  const KitchenPage({super.key, required this.visible});

  /// On screen. Coming back asks again: the pantry may have changed on the
  /// shopping list, and a plan day may have passed.
  final bool visible;

  @override
  State<KitchenPage> createState() => _KitchenPageState();
}

class _KitchenPageState extends State<KitchenPage> {
  final KitchenController _c = KitchenController();

  @override
  void initState() {
    super.initState();
    _c.refresh();
  }

  @override
  void didUpdateWidget(KitchenPage old) {
    super.didUpdateWidget(old);
    if (widget.visible && !old.visible) _c.refresh();
  }

  @override
  void dispose() {
    _c.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return AnimatedBuilder(
      animation: _c,
      builder: (context, _) {
        final st = _c.state;
        return ColoredBox(
          color: t.nCanvas,
          child: Column(
            children: [
              _Header(c: _c, st: st),
              if (_c.busy)
                const LinearProgressIndicator(minHeight: 2, color: kKitchen)
              else
                const SizedBox(height: 2),
              if (_c.notice.isNotEmpty)
                Strip(
                  icon: Icons.check_circle_outline,
                  tint: kKitchen,
                  text: _c.notice,
                  onClose: _c.dismissNotice,
                ),
              if (_c.error != null && st != null)
                Strip(
                  icon: Icons.error_outline,
                  tint: Tokens.error,
                  text: plainError(_c.error!),
                  onClose: _c.clearError,
                ),
              Expanded(
                child: st == null
                    ? FirstLoad(error: _c.error, onRetry: _c.refresh)
                    : switch (st.tab) {
                        'recipe' => _RecipeView(c: _c, st: st),
                        'cook' => CookView(c: _c, st: st),
                        'plan' => PlanView(c: _c, st: st),
                        'shop' => ShopList(c: _c, st: st),
                        'pantry' => PantryView(c: _c, st: st),
                        _ => _RecipesView(c: _c, st: st),
                      },
              ),
            ],
          ),
        );
      },
    );
  }
}

// ------------------------------------------------------------------ header --

class _Header extends StatefulWidget {
  const _Header({required this.c, required this.st});

  final KitchenController c;
  final KitchenState? st;

  @override
  State<_Header> createState() => _HeaderState();
}

class _HeaderState extends State<_Header> {
  final TextEditingController _q = TextEditingController();

  @override
  void didUpdateWidget(_Header old) {
    super.didUpdateWidget(old);
    final q = widget.st?.query ?? '';
    if (q.isEmpty && _q.text.isNotEmpty && old.st?.query != q) _q.clear();
  }

  @override
  void dispose() {
    _q.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final c = widget.c;
    final st = widget.st;
    final tab = st?.tab ?? 'recipes';
    final r = context.skin.controlRadius ?? 10;
    return Container(
      height: 64,
      padding: const EdgeInsets.fromLTRB(22, 0, 20, 0),
      decoration: BoxDecoration(
        color: t.panel,
        border: Border(bottom: BorderSide(color: t.nHair)),
      ),
      child: Row(
        children: [
          Container(
            width: 32,
            height: 32,
            decoration: BoxDecoration(
              color: kKitchen.withValues(alpha: 0.17),
              borderRadius: BorderRadius.circular(10),
            ),
            child: const Icon(Icons.restaurant_outlined,
                size: 18, color: kKitchen),
          ),
          const SizedBox(width: 10),
          Text('Kitchen',
              style: TextStyle(
                  fontSize: 19, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(width: 14),
          Expanded(
            child: SingleChildScrollView(
              scrollDirection: Axis.horizontal,
              child: Container(
                padding: const EdgeInsets.all(3),
                decoration: BoxDecoration(
                  color: t.nChip,
                  borderRadius: BorderRadius.circular(r + 3),
                ),
                child: Row(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    for (final f in keepTabs('kitchen', kitchenTabs, (f) => f.id,
                        active: (f) => tab == f.id))
                      _TabButton(
                        label: f.label,
                        on: f.id == tab,
                        count: f.id == 'shop' ? (st?.shopLeft ?? 0) : 0,
                        radius: r,
                        onTap: () => c.send(KitchenCmd.setTab(tab: f.id)),
                      ),
                  ],
                ),
              ),
            ),
          ),
          const SizedBox(width: 12),
          SizedBox(
            width: 250,
            height: 38,
            child: TextField(
              controller: _q,
              style: TextStyle(fontSize: 13, color: t.nInk),
              textInputAction: TextInputAction.search,
              onChanged: (_) => setState(() {}),
              onSubmitted: (v) => c.send(KitchenCmd.search(text: v)),
              decoration: InputDecoration(
                isDense: true,
                hintText: 'Search recipes or ingredients',
                hintStyle: TextStyle(fontSize: 13, color: t.nInk3),
                prefixIcon: Icon(Icons.search, size: 17, color: t.nInk3),
                suffixIcon: _q.text.isEmpty
                    ? null
                    : IconButton(
                        iconSize: 15,
                        tooltip: 'Clear',
                        icon: const Icon(Icons.close),
                        onPressed: () {
                          _q.clear();
                          c.send(const KitchenCmd.search(text: ''));
                        },
                      ),
                filled: true,
                fillColor: t.nChip,
                contentPadding: const EdgeInsets.symmetric(vertical: 10),
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(r),
                  borderSide: BorderSide.none,
                ),
              ),
            ),
          ),
          const SizedBox(width: 10),
          FilledButton.icon(
            style: FilledButton.styleFrom(backgroundColor: kKitchen),
            onPressed: st == null ? null : () => showAddRecipe(context, c),
            icon: const Icon(Icons.add, size: 17),
            label: const Text('Add recipe'),
          ),
        ],
      ),
    );
  }
}

class _TabButton extends StatelessWidget {
  const _TabButton({
    required this.label,
    required this.on,
    required this.count,
    required this.radius,
    required this.onTap,
  });

  final String label;
  final bool on;
  final int count;
  final double radius;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: on ? kKitchen : Colors.transparent,
      borderRadius: BorderRadius.circular(radius),
      child: InkWell(
        borderRadius: BorderRadius.circular(radius),
        onTap: onTap,
        child: Container(
          height: 32,
          padding: const EdgeInsets.symmetric(horizontal: 14),
          alignment: Alignment.center,
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(label,
                  style: TextStyle(
                      fontSize: 13,
                      fontWeight: FontWeight.w600,
                      color: on ? Colors.white : t.nInk2)),
              if (count > 0) ...[
                const SizedBox(width: 7),
                Container(
                  constraints: const BoxConstraints(minWidth: 18),
                  height: 18,
                  padding: const EdgeInsets.symmetric(horizontal: 5),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: on ? Colors.white : kKitchen,
                    borderRadius: BorderRadius.circular(99),
                  ),
                  child: Text('$count',
                      style: TextStyle(
                          fontSize: 11,
                          fontWeight: FontWeight.w700,
                          color: on ? kKitchen : Colors.white)),
                ),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

// ----------------------------------------------------------------- recipes --

class _RecipesView extends StatelessWidget {
  const _RecipesView({required this.c, required this.st});

  final KitchenController c;
  final KitchenState st;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return ListView(
      padding: const EdgeInsets.fromLTRB(24, 18, 24, 40),
      children: [
        _LinkBar(c: c),
        const SizedBox(height: 6),
        Text(
          'Only the ingredients, steps and photo are kept: no life story, no pop-ups. Or read in a photo of a cookbook page.',
          style: TextStyle(fontSize: 12, color: t.nInk3),
        ),
        const SizedBox(height: 16),
        Wrap(
          spacing: 8,
          runSpacing: 8,
          children: [
            for (final f in st.filters)
              _FilterChip(
                label: f.label,
                n: f.n,
                on: st.filter == f.id,
                onTap: () => c.send(KitchenCmd.setFilter(filter: f.id)),
              ),
          ],
        ),
        const SizedBox(height: 18),
        if (st.query.isNotEmpty)
          Padding(
            padding: const EdgeInsets.only(bottom: 12),
            child: Text(
                '${plural(st.recipes.length, 'recipe')} with “${st.query}”',
                style: TextStyle(fontSize: 13, color: t.nInk2)),
          ),
        if (st.total == 0)
          const Quiet(
            icon: Icons.menu_book_outlined,
            title: 'No recipes yet',
            body:
                'Paste a link to any recipe page above — nearly every recipe site publishes the recipe in a form Tulipix can read — or add one by hand.',
          )
        else if (st.recipes.isEmpty)
          const Quiet(
            icon: Icons.filter_alt_off_outlined,
            title: 'Nothing here',
            body: 'No recipe fits this filter yet.',
          )
        else
          Grid(
            min: 170,
            children: [
              for (final r in st.recipes) _RecipeTile(c: c, r: r),
            ],
          ),
      ],
    );
  }
}

class _LinkBar extends StatefulWidget {
  const _LinkBar({required this.c});

  final KitchenController c;

  @override
  State<_LinkBar> createState() => _LinkBarState();
}

class _LinkBarState extends State<_LinkBar> {
  final TextEditingController _url = TextEditingController();

  @override
  void dispose() {
    _url.dispose();
    super.dispose();
  }

  Future<void> _save() async {
    final v = _url.text.trim();
    if (v.isEmpty) return;
    final err = await widget.c.attempt(KitchenCmd.addFromLink(url: v));
    if (!mounted) return;
    if (err == null) {
      _url.clear();
    } else {
      widget.c.say(err);
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: cardDeco(context),
      padding: const EdgeInsets.fromLTRB(8, 6, 6, 6),
      child: Row(
        children: [
          Container(
            width: 30,
            height: 30,
            decoration: BoxDecoration(
              color: kKitchen.withValues(alpha: 0.14),
              borderRadius: BorderRadius.circular(8),
            ),
            child: const Icon(Icons.link, size: 16, color: kKitchen),
          ),
          const SizedBox(width: 10),
          Expanded(
            child: TextField(
              controller: _url,
              onSubmitted: (_) => _save(),
              style: TextStyle(
                  fontFamily: 'monospace', fontSize: 13, color: t.nInk),
              decoration: InputDecoration(
                isDense: true,
                border: InputBorder.none,
                hintText: 'Paste a recipe link',
                hintStyle: TextStyle(fontSize: 13, color: t.nInk3),
              ),
            ),
          ),
          IconButton(
            tooltip: 'From a photo of a cookbook page',
            onPressed: () => addFromPhoto(context, widget.c),
            icon: Icon(Icons.photo_camera_outlined, size: 19, color: t.nInk2),
          ),
          const SizedBox(width: 4),
          FilledButton.icon(
            style: FilledButton.styleFrom(
                backgroundColor: kKitchen,
                visualDensity: VisualDensity.compact),
            onPressed: widget.c.busy ? null : _save,
            icon: const Icon(Icons.download_outlined, size: 16),
            label: const Text('Save recipe'),
          ),
        ],
      ),
    );
  }
}

class _FilterChip extends StatelessWidget {
  const _FilterChip({
    required this.label,
    required this.n,
    required this.on,
    required this.onTap,
  });

  final String label;
  final int n;
  final bool on;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Material(
      color: on ? kKitchen.withValues(alpha: 0.16) : t.nChip,
      shape: StadiumBorder(
          side: BorderSide(
              color: on ? kKitchen.withValues(alpha: 0.5) : t.nHair)),
      child: InkWell(
        customBorder: const StadiumBorder(),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
          child: Text.rich(TextSpan(children: [
            TextSpan(
                text: label,
                style: TextStyle(
                    fontSize: 12.5,
                    fontWeight: FontWeight.w600,
                    color: on ? kKitchen : t.nInk2)),
            TextSpan(
                text: '  $n',
                style: TextStyle(fontSize: 11, color: t.nInk3)),
          ])),
        ),
      ),
    );
  }
}

class _RecipeTile extends StatelessWidget {
  const _RecipeTile({required this.c, required this.r});

  final KitchenController c;
  final RecipeCard r;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final sub = [
      if (r.time.isNotEmpty) r.time,
      if (r.category.isNotEmpty) r.category,
    ].join(' · ');
    return InkWell(
      borderRadius: BorderRadius.circular(12),
      onTap: () => c.send(KitchenCmd.open(id: r.id)),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          AspectRatio(
            aspectRatio: 4 / 3,
            child: ClipRRect(
              borderRadius: BorderRadius.circular(12),
              child: Stack(
                fit: StackFit.expand,
                children: [
                  Art(path: r.image, seed: r.id),
                  if (r.favourite)
                    const Positioned(
                      right: 8,
                      top: 8,
                      child: _Badge(icon: Icons.favorite, tint: Color(0xFFF43F5E)),
                    ),
                  if (r.canMake)
                    const Positioned(
                      left: 8,
                      top: 8,
                      child: _Badge(icon: Icons.kitchen_outlined, tint: kKitchen),
                    ),
                ],
              ),
            ),
          ),
          const SizedBox(height: 8),
          Text(r.title,
              maxLines: 2,
              overflow: TextOverflow.ellipsis,
              style: TextStyle(
                  fontSize: 13.5, fontWeight: FontWeight.w700, color: t.nInk)),
          if (sub.isNotEmpty) ...[
            const SizedBox(height: 2),
            Text(sub, style: TextStyle(fontSize: 12, color: t.nInk3)),
          ],
        ],
      ),
    );
  }
}

class _Badge extends StatelessWidget {
  const _Badge({required this.icon, required this.tint});

  final IconData icon;
  final Color tint;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: 24,
      height: 24,
      decoration: BoxDecoration(
        color: Colors.black.withValues(alpha: 0.45),
        shape: BoxShape.circle,
      ),
      child: Icon(icon, size: 13, color: tint),
    );
  }
}

// ------------------------------------------------------------------ recipe --

class _RecipeView extends StatelessWidget {
  const _RecipeView({required this.c, required this.st});

  final KitchenController c;
  final KitchenState st;

  @override
  Widget build(BuildContext context) {
    final r = st.open;
    if (r == null) {
      return const Padding(
        padding: EdgeInsets.all(24),
        child: Quiet(
          icon: Icons.menu_book_outlined,
          title: 'No recipe open',
          body: 'Pick one on Recipes, or add one.',
        ),
      );
    }
    return LayoutBuilder(builder: (context, box) {
      final wide = box.maxWidth >= 980;
      final ingredients = _Ingredients(c: c, r: r);
      final method = _Method(r: r);
      return ListView(
        padding: const EdgeInsets.only(bottom: 40),
        children: [
          _Hero(c: c, r: r),
          Padding(
            padding: const EdgeInsets.fromLTRB(24, 18, 24, 0),
            child: wide
                ? Row(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Expanded(flex: 5, child: ingredients),
                      const SizedBox(width: 16),
                      Expanded(flex: 7, child: method),
                    ],
                  )
                : Column(
                    children: [ingredients, const SizedBox(height: 16), method],
                  ),
          ),
        ],
      );
    });
  }
}

class _Hero extends StatelessWidget {
  const _Hero({required this.c, required this.r});

  final KitchenController c;
  final RecipeView r;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    final meta = <(IconData, String)>[
      if (r.time.isNotEmpty) (Icons.schedule, r.time),
      if (r.category.isNotEmpty) (Icons.sell_outlined, r.category),
      if (r.host.isNotEmpty) (Icons.public, r.host),
      if (r.cooked > 0) (Icons.star_outline, 'Cooked ${plural(r.cooked, 'time')}'),
    ];
    return SizedBox(
      height: 250,
      child: Stack(
        fit: StackFit.expand,
        children: [
          Art(path: r.image, seed: r.id),
          DecoratedBox(
            decoration: BoxDecoration(
              gradient: LinearGradient(
                begin: Alignment.topCenter,
                end: Alignment.bottomCenter,
                stops: const [0.25, 1],
                colors: [t.nCanvas.withValues(alpha: 0), t.nCanvas],
              ),
            ),
          ),
          Positioned(
            left: 16,
            top: 14,
            child: _DarkPill(
              icon: Icons.chevron_left,
              text: 'Recipes',
              onTap: () => c.send(const KitchenCmd.setTab(tab: 'recipes')),
            ),
          ),
          Positioned(
            left: 24,
            right: 24,
            bottom: 8,
            child: Row(
              crossAxisAlignment: CrossAxisAlignment.end,
              children: [
                Expanded(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      Text(r.title,
                          maxLines: 2,
                          overflow: TextOverflow.ellipsis,
                          style: TextStyle(
                              fontFamily: kSerif,
                              fontSize: 34,
                              height: 1.1,
                              fontWeight: FontWeight.w700,
                              color: t.nInk)),
                      const SizedBox(height: 6),
                      Wrap(
                        spacing: 14,
                        children: [
                          for (final (icon, text) in meta)
                            Row(
                              mainAxisSize: MainAxisSize.min,
                              children: [
                                Icon(icon, size: 13, color: t.nInk3),
                                const SizedBox(width: 4),
                                Text(text,
                                    style: TextStyle(
                                        fontSize: 12, color: t.nInk2)),
                              ],
                            ),
                        ],
                      ),
                    ],
                  ),
                ),
                const SizedBox(width: 12),
                OutlinedButton.icon(
                  onPressed: () =>
                      c.send(KitchenCmd.toggleFavourite(id: r.id)),
                  icon: Icon(
                      r.favourite ? Icons.favorite : Icons.favorite_border,
                      size: 16,
                      color: r.favourite ? const Color(0xFFF43F5E) : null),
                  label: const Text('Favourite'),
                ),
                const SizedBox(width: 8),
                OutlinedButton.icon(
                  onPressed: () => planIt(context, c, r.id),
                  icon: const Icon(Icons.event_outlined, size: 16),
                  label: const Text('Plan it'),
                ),
                const SizedBox(width: 8),
                PopupMenuButton<String>(
                  tooltip: 'More',
                  icon: Icon(Icons.more_horiz, color: t.nInk2),
                  onSelected: (v) async {
                    if (v == 'edit') await editRecipe(context, c, r.id);
                    if (v == 'delete' && context.mounted) {
                      if (await confirm(context, 'Delete ${r.title}?',
                          'It comes off the plan too. Nothing else is touched.')) {
                        await c.send(KitchenCmd.deleteRecipe(id: r.id));
                      }
                    }
                  },
                  itemBuilder: (_) => const [
                    PopupMenuItem(value: 'edit', child: Text('Edit recipe')),
                    PopupMenuItem(value: 'delete', child: Text('Delete recipe')),
                  ],
                ),
                const SizedBox(width: 8),
                FilledButton.icon(
                  style: FilledButton.styleFrom(backgroundColor: kKitchen),
                  onPressed: r.steps.isEmpty
                      ? null
                      : () => c.send(const KitchenCmd.startCook()),
                  icon: const Icon(Icons.play_arrow, size: 18),
                  label: const Text('Start cooking'),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _DarkPill extends StatelessWidget {
  const _DarkPill({required this.icon, required this.text, required this.onTap});

  final IconData icon;
  final String text;
  final VoidCallback onTap;

  @override
  Widget build(BuildContext context) {
    return Material(
      color: Colors.black.withValues(alpha: 0.55),
      borderRadius: BorderRadius.circular(8),
      child: InkWell(
        borderRadius: BorderRadius.circular(8),
        onTap: onTap,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(6, 5, 10, 5),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Icon(icon, size: 16, color: Colors.white),
              const SizedBox(width: 2),
              Text(text,
                  style: const TextStyle(
                      fontSize: 12,
                      fontWeight: FontWeight.w600,
                      color: Colors.white)),
            ],
          ),
        ),
      ),
    );
  }
}

class _Ingredients extends StatelessWidget {
  const _Ingredients({required this.c, required this.r});

  final KitchenController c;
  final RecipeView r;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: cardDeco(context),
      padding: const EdgeInsets.fromLTRB(14, 10, 14, 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Text('Ingredients',
                  style: TextStyle(
                      fontSize: 14,
                      fontWeight: FontWeight.w700,
                      color: t.nInk)),
              const Spacer(),
              _Stepper(
                value: r.servings,
                onChanged: (v) =>
                    c.send(KitchenCmd.setServings(servings: v)),
              ),
              const SizedBox(width: 6),
              Text('servings', style: TextStyle(fontSize: 12, color: t.nInk3)),
            ],
          ),
          const SizedBox(height: 6),
          for (final i in r.ingredients) ...[
            Divider(height: 1, color: t.nHair),
            _IngLine(c: c, i: i),
          ],
          if (r.missing > 0) ...[
            const SizedBox(height: 12),
            Align(
              alignment: Alignment.centerLeft,
              child: OutlinedButton.icon(
                onPressed: () => c.send(const KitchenCmd.addMissing()),
                icon: const Icon(Icons.playlist_add, size: 17),
                label: Text(
                    'Add ${plural(r.missing, 'missing item')} to the shopping list'),
              ),
            ),
          ],
          const SizedBox(height: 6),
          Text('Tick what you have: the kitchen remembers it.',
              style: TextStyle(fontSize: 11.5, color: t.nInk3)),
        ],
      ),
    );
  }
}

class _IngLine extends StatelessWidget {
  const _IngLine({required this.c, required this.i});

  final KitchenController c;
  final IngRow i;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Row(
        children: [
          SizedBox(
            width: 30,
            child: i.staple
                ? const SizedBox.shrink()
                : Checkbox(
                    value: i.inStock,
                    activeColor: kKitchen,
                    visualDensity: VisualDensity.compact,
                    onChanged: (v) => c.send(
                        KitchenCmd.setStock(key: i.key, have: v ?? false)),
                  ),
          ),
          const SizedBox(width: 6),
          SizedBox(
            width: 76,
            child: Text(i.amount,
                style: TextStyle(
                    fontSize: 13,
                    fontWeight: FontWeight.w700,
                    color: t.nInk)),
          ),
          Expanded(
            child: Text.rich(TextSpan(children: [
              TextSpan(
                  text: i.name,
                  style: TextStyle(fontSize: 13.5, color: t.nInk)),
              if (i.note.isNotEmpty)
                TextSpan(
                    text: ', ${i.note}',
                    style: TextStyle(fontSize: 12.5, color: t.nInk3)),
            ])),
          ),
          if (!i.inStock)
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 6, vertical: 2),
              decoration: BoxDecoration(
                color: const Color(0xFFF97316).withValues(alpha: 0.12),
                borderRadius: BorderRadius.circular(5),
              ),
              child: const Text('not in stock',
                  style: TextStyle(
                      fontSize: 10.5,
                      fontWeight: FontWeight.w600,
                      color: Color(0xFFEA580C))),
            ),
        ],
      ),
    );
  }
}

class _Stepper extends StatelessWidget {
  const _Stepper({required this.value, required this.onChanged});

  final int value;
  final ValueChanged<int> onChanged;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    Widget btn(IconData icon, int to) => Material(
          color: t.nChip,
          borderRadius: BorderRadius.circular(7),
          child: InkWell(
            borderRadius: BorderRadius.circular(7),
            onTap: to < 1 ? null : () => onChanged(to),
            child: SizedBox(
                width: 28, height: 26, child: Icon(icon, size: 15, color: t.nInk2)),
          ),
        );
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        btn(Icons.remove, value - 1),
        SizedBox(
          width: 30,
          child: Text('$value',
              textAlign: TextAlign.center,
              style: TextStyle(
                  fontSize: 15, fontWeight: FontWeight.w800, color: t.nInk)),
        ),
        btn(Icons.add, value + 1),
      ],
    );
  }
}

class _Method extends StatelessWidget {
  const _Method({required this.r});

  final RecipeView r;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      decoration: cardDeco(context),
      padding: const EdgeInsets.fromLTRB(14, 12, 14, 14),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text('Method',
              style: TextStyle(
                  fontSize: 14, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 10),
          if (r.steps.isEmpty)
            Text('No steps yet — Edit recipe to add them.',
                style: TextStyle(fontSize: 13, color: t.nInk3)),
          for (var n = 0; n < r.steps.length; n++)
            Padding(
              padding: const EdgeInsets.only(bottom: 14),
              child: Row(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Container(
                    width: 22,
                    height: 22,
                    alignment: Alignment.center,
                    decoration: const BoxDecoration(
                        color: kKitchen, shape: BoxShape.circle),
                    child: Text('${n + 1}',
                        style: const TextStyle(
                            fontSize: 11,
                            fontWeight: FontWeight.w800,
                            color: Colors.white)),
                  ),
                  const SizedBox(width: 12),
                  Expanded(
                    child: Column(
                      crossAxisAlignment: CrossAxisAlignment.start,
                      children: [
                        Text(r.steps[n].text,
                            style: TextStyle(
                                fontFamily: kSerif,
                                fontSize: 15.5,
                                height: 1.5,
                                color: t.nInk)),
                        if (r.steps[n].timers.isNotEmpty) ...[
                          const SizedBox(height: 8),
                          Wrap(
                            spacing: 6,
                            children: [
                              for (final tm in r.steps[n].timers)
                                Container(
                                  padding: const EdgeInsets.symmetric(
                                      horizontal: 8, vertical: 4),
                                  decoration: BoxDecoration(
                                    color: t.nChip,
                                    borderRadius: BorderRadius.circular(7),
                                    border: Border.all(color: t.nHair),
                                  ),
                                  child: Row(
                                    mainAxisSize: MainAxisSize.min,
                                    children: [
                                      Icon(Icons.timer_outlined,
                                          size: 13, color: t.nInk2),
                                      const SizedBox(width: 4),
                                      Text('${span(tm.secs)} timer',
                                          style: TextStyle(
                                              fontSize: 11.5,
                                              fontWeight: FontWeight.w600,
                                              color: t.nInk2)),
                                    ],
                                  ),
                                ),
                            ],
                          ),
                        ],
                      ],
                    ),
                  ),
                ],
              ),
            ),
        ],
      ),
    );
  }
}

// ----------------------------------------------------------------- dialogs --

/// Add recipe: a link, a photo of a page, or by hand.
Future<void> showAddRecipe(BuildContext context, KitchenController c) async {
  final how = await showDialog<String>(
    context: context,
    builder: (ctx) => SimpleDialog(
      title: const Text('Add a recipe'),
      children: [
        SimpleDialogOption(
          onPressed: () => Navigator.pop(ctx, 'link'),
          child: const ListTile(
            leading: Icon(Icons.link),
            title: Text('From a link'),
            subtitle: Text('Any recipe page — only the recipe is kept'),
          ),
        ),
        SimpleDialogOption(
          onPressed: () => Navigator.pop(ctx, 'photo'),
          child: const ListTile(
            leading: Icon(Icons.photo_camera_outlined),
            title: Text('From a photo'),
            subtitle: Text('A cookbook page, read on this computer'),
          ),
        ),
        SimpleDialogOption(
          onPressed: () => Navigator.pop(ctx, 'hand'),
          child: const ListTile(
            leading: Icon(Icons.edit_outlined),
            title: Text('By hand'),
            subtitle: Text('Type or paste it in'),
          ),
        ),
      ],
    ),
  );
  if (!context.mounted || how == null) return;
  switch (how) {
    case 'link':
      final url = await askLine(context, 'Recipe link', 'https://…');
      if (url == null || url.trim().isEmpty) return;
      final err = await c.attempt(KitchenCmd.addFromLink(url: url));
      if (err != null) c.say(err);
    case 'photo':
      await addFromPhoto(context, c);
    default:
      await editRecipe(context, c, 0);
  }
}

Future<void> addFromPhoto(BuildContext context, KitchenController c) async {
  final path = await dialogPickFile(
    title: 'A photo of a cookbook page',
    initial: '',
    label: 'Images',
    extensions: const ['jpg', 'jpeg', 'png', 'webp', 'tif', 'tiff', 'bmp'],
  );
  if (path == null || path.isEmpty || !context.mounted) return;
  c.say('Reading the page…');
  try {
    final d = await kitchenOcr(path: path);
    if (!context.mounted) return;
    await showEditor(context, c, d);
  } catch (e) {
    c.say(plainError(e));
  }
}

Future<void> editRecipe(BuildContext context, KitchenController c, int id) async {
  try {
    final d = await kitchenDraft(id: id);
    if (!context.mounted) return;
    await showEditor(context, c, d);
  } catch (e) {
    c.say(plainError(e));
  }
}

/// The recipe as lines: one ingredient per line, one step per line.
Future<void> showEditor(
    BuildContext context, KitchenController c, RecipeDraft d) {
  final title = TextEditingController(text: d.title);
  final minutes = TextEditingController(text: d.minutes > 0 ? '${d.minutes}' : '');
  final servings = TextEditingController(text: '${d.servings}');
  final category = TextEditingController(text: d.category);
  final ings = TextEditingController(text: d.ingredients);
  final steps = TextEditingController(text: d.steps);
  final url = TextEditingController(text: d.sourceUrl);
  String? error;
  return showDialog<void>(
    context: context,
    builder: (ctx) => StatefulBuilder(
      builder: (ctx, set) => AlertDialog(
        title: Text(d.id == 0 ? 'New recipe' : 'Edit recipe'),
        content: SizedBox(
          width: 640,
          child: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                TextField(
                    controller: title,
                    autofocus: d.title.isEmpty,
                    decoration: const InputDecoration(labelText: 'Name')),
                Row(
                  children: [
                    Expanded(
                      child: TextField(
                          controller: minutes,
                          keyboardType: TextInputType.number,
                          decoration:
                              const InputDecoration(labelText: 'Minutes')),
                    ),
                    const SizedBox(width: 12),
                    Expanded(
                      child: TextField(
                          controller: servings,
                          keyboardType: TextInputType.number,
                          decoration:
                              const InputDecoration(labelText: 'Serves')),
                    ),
                    const SizedBox(width: 12),
                    Expanded(
                      flex: 2,
                      child: TextField(
                          controller: category,
                          decoration: const InputDecoration(
                              labelText: 'Kind',
                              hintText: 'Vegetarian, Baking, Fish…')),
                    ),
                  ],
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: ings,
                  minLines: 5,
                  maxLines: 12,
                  decoration: const InputDecoration(
                    labelText: 'Ingredients, one per line',
                    hintText: '2 tbsp olive oil\n800 g chopped tomatoes',
                    alignLabelWithHint: true,
                  ),
                ),
                const SizedBox(height: 12),
                TextField(
                  controller: steps,
                  minLines: 5,
                  maxLines: 14,
                  decoration: const InputDecoration(
                    labelText: 'Method, one step per line',
                    hintText: 'Simmer for 10 minutes until thick.',
                    alignLabelWithHint: true,
                  ),
                ),
                TextField(
                    controller: url,
                    decoration:
                        const InputDecoration(labelText: 'Where it came from')),
                if (error != null) ...[
                  const SizedBox(height: 10),
                  Text(error!,
                      style: const TextStyle(color: Tokens.error, fontSize: 12.5)),
                ],
              ],
            ),
          ),
        ),
        actions: [
          TextButton(
              onPressed: () => Navigator.pop(ctx),
              child: const Text('Cancel')),
          FilledButton(
            style: FilledButton.styleFrom(backgroundColor: kKitchen),
            onPressed: () async {
              final err = await c.attempt(KitchenCmd.saveRecipe(
                id: d.id,
                title: title.text,
                minutes: int.tryParse(minutes.text.trim()) ?? 0,
                servings: int.tryParse(servings.text.trim()) ?? 4,
                category: category.text,
                ingredients: ings.text,
                steps: steps.text,
                sourceUrl: url.text,
              ));
              if (!ctx.mounted) return;
              if (err == null) {
                Navigator.pop(ctx);
              } else {
                set(() => error = err);
              }
            },
            child: const Text('Save'),
          ),
        ],
      ),
    ),
  );
}

Future<String?> askLine(BuildContext context, String title, String hint,
    {String initial = ''}) {
  final ctl = TextEditingController(text: initial);
  return showDialog<String>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: SizedBox(
        width: 420,
        child: TextField(
          controller: ctl,
          autofocus: true,
          decoration: InputDecoration(hintText: hint),
          onSubmitted: (v) => Navigator.pop(ctx, v),
        ),
      ),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx), child: const Text('Cancel')),
        FilledButton(
          style: FilledButton.styleFrom(backgroundColor: kKitchen),
          onPressed: () => Navigator.pop(ctx, ctl.text),
          child: const Text('OK'),
        ),
      ],
    ),
  );
}

Future<bool> confirm(BuildContext context, String title, String body) async {
  final ok = await showDialog<bool>(
    context: context,
    builder: (ctx) => AlertDialog(
      title: Text(title),
      content: Text(body),
      actions: [
        TextButton(
            onPressed: () => Navigator.pop(ctx, false),
            child: const Text('Keep')),
        FilledButton(
          style: FilledButton.styleFrom(backgroundColor: Tokens.error),
          onPressed: () => Navigator.pop(ctx, true),
          child: const Text('Delete'),
        ),
      ],
    ),
  );
  return ok ?? false;
}

// ------------------------------------------------------------------ shared --

Decoration cardDeco(BuildContext context, {double? radius}) {
  final t = context.tokens;
  final r = radius ?? context.skin.panelRadius ?? 14;
  return context.skin.surface(SurfaceRole.card, radius: r) ??
      BoxDecoration(
        color: t.nCard,
        borderRadius: BorderRadius.circular(r),
        border: Border.all(color: t.nHair),
      );
}

/// A recipe's photo, or the deck's plate on a coloured ground when it has
/// none.
class Art extends StatelessWidget {
  const Art({super.key, required this.path, required this.seed});

  final String path;
  final int seed;

  @override
  Widget build(BuildContext context) {
    final plate = _Plate(seed: seed);
    if (path.isEmpty) return plate;
    return FileArt(
      path,
      gaplessPlayback: true,
      errorBuilder: (_, __, ___) => plate,
    );
  }
}

class _Plate extends StatelessWidget {
  const _Plate({required this.seed});

  final int seed;

  @override
  Widget build(BuildContext context) {
    final (ground, food) = plateColours(seed);
    return LayoutBuilder(builder: (context, box) {
      final d = (box.maxWidth < box.maxHeight ? box.maxWidth : box.maxHeight) * 0.62;
      return DecoratedBox(
        decoration: BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topLeft,
            end: Alignment.bottomRight,
            colors: [ground, Color.lerp(ground, Colors.black, 0.35)!],
          ),
        ),
        child: Center(
          child: Container(
            width: d,
            height: d,
            decoration: const BoxDecoration(
                color: Color(0xFFF5F5F4), shape: BoxShape.circle),
            alignment: Alignment.center,
            child: Container(
              width: d * 0.72,
              height: d * 0.72,
              decoration: BoxDecoration(color: food, shape: BoxShape.circle),
            ),
          ),
        ),
      );
    });
  }
}

/// Cards in columns of at least [min] wide, filling the row.
class Grid extends StatelessWidget {
  const Grid({super.key, required this.children, required this.min, this.gap = 14});

  final List<Widget> children;
  final double min;
  final double gap;

  @override
  Widget build(BuildContext context) {
    return LayoutBuilder(
      builder: (context, box) {
        final cols = ((box.maxWidth + gap) / (min + gap)).floor().clamp(1, 99);
        final w = (box.maxWidth - gap * (cols - 1)) / cols;
        return Wrap(
          spacing: gap,
          runSpacing: gap + 4,
          children: [for (final c in children) SizedBox(width: w, child: c)],
        );
      },
    );
  }
}

class Quiet extends StatelessWidget {
  const Quiet({super.key, required this.icon, required this.title, required this.body});

  final IconData icon;
  final String title;
  final String body;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      padding: const EdgeInsets.all(28),
      decoration: cardDeco(context),
      child: Column(
        children: [
          Icon(icon, size: 30, color: t.nInk3),
          const SizedBox(height: 10),
          Text(title,
              style: TextStyle(
                  fontSize: 15, fontWeight: FontWeight.w700, color: t.nInk)),
          const SizedBox(height: 6),
          Text(body,
              textAlign: TextAlign.center,
              style: TextStyle(fontSize: 12.5, height: 1.5, color: t.nInk2)),
        ],
      ),
    );
  }
}

class Strip extends StatelessWidget {
  const Strip({
    super.key,
    required this.icon,
    required this.tint,
    required this.text,
    required this.onClose,
  });

  final IconData icon;
  final Color tint;
  final String text;
  final VoidCallback onClose;

  @override
  Widget build(BuildContext context) {
    final t = context.tokens;
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.fromLTRB(22, 6, 8, 6),
      color: tint.withValues(alpha: 0.10),
      child: Row(
        children: [
          Icon(icon, size: 16, color: tint),
          const SizedBox(width: 10),
          Expanded(
            child: Text(text,
                maxLines: 2,
                overflow: TextOverflow.ellipsis,
                style: TextStyle(fontSize: 12.5, color: t.nInk)),
          ),
          IconButton(
            iconSize: 16,
            tooltip: 'Dismiss',
            onPressed: onClose,
            icon: const Icon(Icons.close),
          ),
        ],
      ),
    );
  }
}
