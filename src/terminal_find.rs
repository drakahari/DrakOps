//! A bounded, on-demand search of one tab's retained terminal output.
use crate::terminal;

// Queries and scans stay local and only run after an explicit find action.
const QUERY_LIMIT: usize = 256;
const SCAN_BYTES: usize = 4 * 1024 * 1024;
const HIT_LIMIT: usize = 4096;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Direction { Next, Previous }

#[derive(Debug, PartialEq, Eq)]
struct Hit {
    row: usize,
    // Offset in the transformed logical row, used to cycle through several hits
    // on the same visible row.
    column: usize,
}

#[derive(Default)]
pub(crate) struct Results {
    hits: Vec<Hit>,
    live: usize,
    offset: usize,
    limited: bool,
}

pub(crate) fn scan(screen: &mut vt100::Screen, query: &str) -> Results {
    let needle=query.to_lowercase();
    if needle.is_empty() || query.chars().count()>QUERY_LIMIT || query.chars().any(char::is_control) || screen.alternate_screen() {
        return Results::default();
    }
    let offset=screen.scrollback();
    let live=terminal::available_scrollback(screen);
    let (height,width)=screen.size();
    let height=usize::from(height);
    let mut text=String::new();
    let mut starts=Vec::new();
    let mut top=0;
    let mut limited=false;
    // Each vt100 viewport is read only once. For a partial final page, skip
    // overlap rather than duplicating rows. Never leave the user's view moved.
    'pages: loop {
        let start=top.min(live);
        screen.set_scrollback(live-start);
        let skip=top-start;
        for (visible,row) in screen.rows(0,width).enumerate().skip(skip) {
            let row=row.to_lowercase();
            if text.len()+row.len()+1>SCAN_BYTES {
                limited=true;
                break 'pages;
            }
            starts.push(text.len());
            text.push_str(&row);
            if !screen.row_wrapped(visible as u16) {text.push('\n');}
        }
        if start==live {break;}
        top=start+height;
    }
    screen.set_scrollback(offset);
    let mut hits=Vec::new();
    for (index,_) in text.match_indices(&needle) {
        if hits.len()==HIT_LIMIT {limited=true;break;}
        let row=starts.partition_point(|start|*start<=index).saturating_sub(1);
        hits.push(Hit {row,column:index-starts[row]});
    }
    Results {hits,live,offset,limited}
}

#[derive(Default)]
pub(crate) struct Find {
    pub(crate) open: bool,
    pub(crate) first_frame: bool,
    pub(crate) query: String,
    pub(crate) status: String,
    previous: Option<(usize,usize)>,
}

impl Find {
    pub(crate) fn highlighted_row(&self) -> Option<u16> {
        self.previous.and_then(|(row,_)|u16::try_from(row).ok())
    }

    pub(crate) fn changed(&mut self) {
        self.previous=None;
        self.status.clear();
    }

    pub(crate) fn apply(&mut self, matches: Results, direction: Direction) -> Option<usize> {
        let Results {hits,live,offset,limited}=matches;
        if hits.is_empty() {
            self.previous=None;
            self.status=if limited {"No matches in the scanned output (search limit reached)."} else {"No matches in retained output."}.into();
            return None;
        }
        let view_top=live-offset;
        let key=(view_top+self.previous.map_or(0,|x|x.0),self.previous.map_or(0,|x|x.1));
        let index=match direction {
            Direction::Next => {
                let next=hits.partition_point(|hit| {
                    let point=(hit.row,hit.column);
                    if self.previous.is_some() {point<=key} else {point<key}
                });
                if next==hits.len() {0} else {next}
            }
            Direction::Previous => {
                let before=hits.partition_point(|hit|(hit.row,hit.column)<key);
                if before==0 {hits.len()-1} else {before-1}
            }
        };
        let found=&hits[index];
        let start=found.row.min(live);
        self.previous=Some((found.row-start,found.column));
        self.status=format!("Match {} of {}{}",index+1,hits.len(),if limited {" (search capped)"} else {""});
        Some(live-start)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::SCROLLBACK;

    #[test]
    fn scans_wrapped_text_without_crossing_explicit_breaks_or_moving_view() {
        let mut parser=vt100::Parser::new(3,6,SCROLLBACK);
        parser.process(b"abcXYZwrapped\r\nleft\r\nright\r\nlast");
        parser.screen_mut().set_scrollback(2);
        let offset=parser.screen().scrollback();
        let result=scan(parser.screen_mut(),"xyzWRAP");
        assert_eq!(result.hits,vec![Hit {row:0,column:3}]);
        assert_eq!(parser.screen().scrollback(),offset);
        assert!(scan(parser.screen_mut(),"leftright").hits.is_empty());
        parser.process(b"\x1b[?1049hmatch");
        assert!(scan(parser.screen_mut(),"match").hits.is_empty());
    }

    #[test]
    fn next_previous_cycle_multiple_hits_and_last_page_without_duplicate_rows() {
        let mut parser=vt100::Parser::new(4,24,SCROLLBACK);
        for line in ["two key KEY","zero","key","zero","zero","end key"] {
            parser.process(format!("{line}\r\n").as_bytes());
        }
        assert_eq!(scan(parser.screen_mut(),"key").hits.len(),4);
        let mut find=Find::default();
        parser.screen_mut().set_scrollback(1);
        let jump=find.apply(scan(parser.screen_mut(),"key"),Direction::Next).unwrap();
        assert_eq!(find.status,"Match 3 of 4");
        parser.screen_mut().set_scrollback(jump);
        let _=find.apply(scan(parser.screen_mut(),"key"),Direction::Next);
        assert_eq!(find.status,"Match 4 of 4");
        let jump=find.apply(scan(parser.screen_mut(),"key"),Direction::Previous).unwrap();
        parser.screen_mut().set_scrollback(jump);
        assert_eq!(find.status,"Match 3 of 4");
    }
}
