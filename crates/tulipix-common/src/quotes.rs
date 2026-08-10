//! The quote shelf for the Welcome home layout's hero.
//!
//! **Bundled, not downloaded.** Two reasons. The app is local-only by decree —
//! a home screen that needs the network to draw its own hero would be the one
//! place that breaks that — and a scraped "top 100 quotes" listicle is somebody
//! else's compiled page. Everything here is a short attributed quotation from a
//! long-dead author, i.e. public domain, kept to roughly one line so it fits the
//! two-line block without eliding.
//!
//! Order is shuffled once per run (see [`shuffled`]); the UI walks the shuffled
//! list one per minute and wraps.

/// `(quotation, author)`.
pub const QUOTES: &[(&str, &str)] = &[
    ("You have power over your mind — not outside events. Realise this, and you will find strength.", "Marcus Aurelius"),
    ("The happiness of your life depends upon the quality of your thoughts.", "Marcus Aurelius"),
    ("Waste no more time arguing what a good man should be. Be one.", "Marcus Aurelius"),
    ("Very little is needed to make a happy life; it is all within yourself.", "Marcus Aurelius"),
    ("If it is not right, do not do it; if it is not true, do not say it.", "Marcus Aurelius"),
    ("We suffer more often in imagination than in reality.", "Seneca"),
    ("Luck is what happens when preparation meets opportunity.", "Seneca"),
    ("It is not that we have a short time to live, but that we waste much of it.", "Seneca"),
    ("Begin at once to live, and count each separate day as a separate life.", "Seneca"),
    ("Difficulties strengthen the mind, as labour does the body.", "Seneca"),
    ("It is not what happens to you, but how you react to it that matters.", "Epictetus"),
    ("First say to yourself what you would be; then do what you have to do.", "Epictetus"),
    ("No man is free who is not master of himself.", "Epictetus"),
    ("Wealth consists not in having great possessions, but in having few wants.", "Epictetus"),
    ("It does not matter how slowly you go as long as you do not stop.", "Confucius"),
    ("The man who moves a mountain begins by carrying away small stones.", "Confucius"),
    ("Real knowledge is to know the extent of one's ignorance.", "Confucius"),
    ("When it is obvious that the goals cannot be reached, adjust the action steps.", "Confucius"),
    ("Everything has beauty, but not everyone sees it.", "Confucius"),
    ("A journey of a thousand miles begins with a single step.", "Lao Tzu"),
    ("Nature does not hurry, yet everything is accomplished.", "Lao Tzu"),
    ("He who knows others is wise; he who knows himself is enlightened.", "Lao Tzu"),
    ("When I let go of what I am, I become what I might be.", "Lao Tzu"),
    ("We are what we repeatedly do. Excellence, then, is not an act but a habit.", "Aristotle"),
    ("Knowing yourself is the beginning of all wisdom.", "Aristotle"),
    ("The whole is greater than the sum of its parts.", "Aristotle"),
    ("Patience is bitter, but its fruit is sweet.", "Aristotle"),
    ("The unexamined life is not worth living.", "Socrates"),
    ("I know one thing: that I know nothing.", "Socrates"),
    ("The secret of change is to focus all of your energy on building the new.", "Socrates"),
    ("Be kind, for everyone you meet is fighting a hard battle.", "Plato"),
    ("The beginning is the most important part of the work.", "Plato"),
    ("No man ever steps in the same river twice.", "Heraclitus"),
    ("Big results require big ambitions.", "Heraclitus"),
    ("A rolling stone gathers no moss.", "Publilius Syrus"),
    ("Practice is the best of all instructors.", "Publilius Syrus"),
    ("While we teach, we learn.", "Seneca the Younger"),
    ("The life of the dead is placed in the memory of the living.", "Cicero"),
    ("A room without books is like a body without a soul.", "Cicero"),
    ("Any man can make mistakes, but only an idiot persists in his error.", "Cicero"),
    ("Dripping water hollows out stone, not through force but through persistence.", "Ovid"),
    ("Be patient and tough; someday this pain will be useful to you.", "Ovid"),
    ("Seize the day, trusting as little as possible in tomorrow.", "Horace"),
    ("He who has begun has half done. Dare to be wise; begin!", "Horace"),
    ("Fortune favours the bold.", "Virgil"),
    ("They can because they think they can.", "Virgil"),
    ("This above all: to thine own self be true.", "William Shakespeare"),
    ("We know what we are, but know not what we may be.", "William Shakespeare"),
    ("Our doubts are traitors, and make us lose the good we oft might win.", "William Shakespeare"),
    ("Nothing is either good or bad, but thinking makes it so.", "William Shakespeare"),
    ("How far that little candle throws his beams!", "William Shakespeare"),
    ("What lies behind us and before us are tiny matters compared to what lies within.", "Ralph Waldo Emerson"),
    ("Do not go where the path may lead; go instead where there is no path.", "Ralph Waldo Emerson"),
    ("The only person you are destined to become is the person you decide to be.", "Ralph Waldo Emerson"),
    ("Write it on your heart that every day is the best day in the year.", "Ralph Waldo Emerson"),
    ("Go confidently in the direction of your dreams. Live the life you have imagined.", "Henry David Thoreau"),
    ("It is not enough to be busy. The question is: what are we busy about?", "Henry David Thoreau"),
    ("Things do not change; we change.", "Henry David Thoreau"),
    ("The secret of getting ahead is getting started.", "Mark Twain"),
    ("Kindness is the language which the deaf can hear and the blind can see.", "Mark Twain"),
    ("Whenever you find yourself on the side of the majority, pause and reflect.", "Mark Twain"),
    ("Continuous improvement is better than delayed perfection.", "Mark Twain"),
    ("Hope is the thing with feathers that perches in the soul.", "Emily Dickinson"),
    ("Keep your face always toward the sunshine, and shadows will fall behind you.", "Walt Whitman"),
    ("Be curious, not judgmental.", "Walt Whitman"),
    ("There is no charm equal to tenderness of heart.", "Jane Austen"),
    ("It isn't what we say or think that defines us, but what we do.", "Jane Austen"),
    ("No one is useless in this world who lightens the burden of another.", "Charles Dickens"),
    ("A day wasted on others is not wasted on one's self.", "Charles Dickens"),
    ("Be yourself; everyone else is already taken.", "Oscar Wilde"),
    ("We are all in the gutter, but some of us are looking at the stars.", "Oscar Wilde"),
    ("The only way to get rid of a temptation is to yield to it.", "Oscar Wilde"),
    ("He who has a why to live can bear almost any how.", "Friedrich Nietzsche"),
    ("That which does not kill us makes us stronger.", "Friedrich Nietzsche"),
    ("Whatever you can do or dream you can, begin it. Boldness has genius in it.", "Johann Wolfgang von Goethe"),
    ("Knowing is not enough; we must apply. Willing is not enough; we must do.", "Johann Wolfgang von Goethe"),
    ("Everyone thinks of changing the world, but no one thinks of changing himself.", "Leo Tolstoy"),
    ("The two most powerful warriors are patience and time.", "Leo Tolstoy"),
    ("Above all, do not lie to yourself.", "Fyodor Dostoevsky"),
    ("Pain and suffering are always inevitable for a large intelligence and a deep heart.", "Fyodor Dostoevsky"),
    ("Well done is better than well said.", "Benjamin Franklin"),
    ("An investment in knowledge pays the best interest.", "Benjamin Franklin"),
    ("Lost time is never found again.", "Benjamin Franklin"),
    ("Whatever you are, be a good one.", "Abraham Lincoln"),
    ("The best way to predict the future is to create it.", "Abraham Lincoln"),
    ("To see a world in a grain of sand, and a heaven in a wild flower.", "William Blake"),
    ("A thing of beauty is a joy for ever.", "John Keats"),
    ("Fear not for the future, weep not for the past.", "Percy Bysshe Shelley"),
    ("There is pleasure in the pathless woods.", "Lord Byron"),
    ("The child is father of the man.", "William Wordsworth"),
    ("We have forty million reasons for failure, but not a single excuse.", "Rudyard Kipling"),
    ("Don't judge each day by the harvest you reap but by the seeds that you plant.", "Robert Louis Stevenson"),
    ("Why, sometimes I've believed as many as six impossible things before breakfast.", "Lewis Carroll"),
    ("It is better to fail in originality than to succeed in imitation.", "Herman Melville"),
    ("Happiness is a butterfly which, when pursued, is just beyond your grasp.", "Nathaniel Hawthorne"),
    ("All that we see or seem is but a dream within a dream.", "Edgar Allan Poe"),
    ("No act of kindness, no matter how small, is ever wasted.", "Aesop"),
    ("Slow and steady wins the race.", "Aesop"),
    ("The wound is the place where the light enters you.", "Rumi"),
    ("Yesterday I was clever, so I wanted to change the world. Today I am wise, so I am changing myself.", "Rumi"),
    ("Opportunities multiply as they are seized.", "Sun Tzu"),
    ("In the midst of chaos, there is also opportunity.", "Sun Tzu"),
    ("The greatest thing in the world is to know how to belong to oneself.", "Michel de Montaigne"),
    ("My life has been filled with terrible misfortunes, most of which never happened.", "Michel de Montaigne"),
    ("Knowledge is power.", "Francis Bacon"),
    ("Small opportunities are often the beginning of great enterprises.", "Demosthenes"),
    ("The heart has its reasons which reason knows nothing of.", "Blaise Pascal"),
    ("Judge a man by his questions rather than by his answers.", "Voltaire"),
    ("Perfect is the enemy of good.", "Voltaire"),
    ("Patience and time do more than strength or passion.", "Jean de La Fontaine"),
    ("Talent hits a target no one else can hit; genius hits a target no one else can see.", "Arthur Schopenhauer"),
    ("Nothing is worth more than this day.", "Johann Wolfgang von Goethe"),
];

/// The quote list in a fresh random order.
///
/// Fisher–Yates over an xorshift seeded from the clock — a shuffle of a hundred
/// strings once a run does not justify pulling `rand` into this crate's tree.
pub fn shuffled() -> Vec<(&'static str, &'static str)> {
    let mut v: Vec<(&'static str, &'static str)> = QUOTES.to_vec();
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15)
        | 1; // xorshift dies on a zero seed
    for i in (1..v.len()).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        v.swap(i, (state % (i as u64 + 1)) as usize);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shuffle_keeps_every_quote_exactly_once() {
        let a = shuffled();
        assert_eq!(a.len(), QUOTES.len());
        for q in QUOTES {
            assert_eq!(a.iter().filter(|x| x.0 == q.0 && x.1 == q.1).count(), 1);
        }
    }
    #[test]
    fn quotes_fit_the_two_line_block() {
        for (text, author) in QUOTES {
            assert!(!text.is_empty() && !author.is_empty());
            assert!(text.chars().count() <= 100, "too long for the hero block: {text}");
        }
    }
}
