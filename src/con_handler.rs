use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::fs::{self, File};
use tokio::io::{self, AsyncBufReadExt, AsyncWrite, BufReader};
use url::Url;

#[cfg(any(feature = "cgi", feature = "scgi"))]
use crate::cgi;
use crate::conn;
use crate::logger;
#[cfg(feature = "proxy")]
use crate::revproxy;
use crate::status::Status;
use crate::util;

type Result<T = ()> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn get_mime(path: &Path) -> String {
    let mut mime = "text/gemini".to_string();
    if path.is_dir() {
        return mime;
    }
    let ext = match path.extension() {
        Some(p) => p.to_str().unwrap(),
        None => return "text/plain".to_string(),
    };

    mime = match new_mime_guess::from_ext(ext).first() {
        Some(m) => m.essence_str().to_string(),
        None => "text/plain".to_string(),
    };

    mime
}

async fn get_binary(mut con: conn::Connection, path: PathBuf, meta: String) -> io::Result<()> {
    let fd = File::open(path).await?;
    let mut reader = BufReader::with_capacity(1024 * 1024, fd);
    con.send_raw(format!("{} {}\r\n", Status::Success as u8, &meta).as_bytes())
        .await?;
    loop {
        let len = {
            let buf = reader.fill_buf().await?;
            con.send_raw(buf).await?;
            buf.len()
        };
        if len == 0 {
            break;
        }
        reader.consume(len);
    }

    futures_util::future::poll_fn(|ctx| std::pin::Pin::new(&mut con.stream).poll_shutdown(ctx))
        .await?;
    Ok(())
}

async fn gen_dir_list(path: PathBuf, u: &url::Url) -> Result<String> {
    let mut dirs: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();

    // needs work
    let mut dir = fs::read_dir(&path).await?;
    while let Some(file) = dir.next_entry().await? {
        let m = file.metadata().await?;
        let perm = m.permissions();
        if perm.mode() & 0o0444 != 0o0444 {
            continue;
        }
        let file = file.path();
        let p = file.strip_prefix(&path).unwrap();
        let ps = match p.to_str() {
            Some(s) => s,
            None => continue,
        };
        let ep = match u.join(ps) {
            Ok(p) => p,
            _ => continue,
        };
        if m.is_dir() {
            dirs.push(format!("=> {}/ {}/\r\n", ep, p.display()));
        } else {
            files.push(format!("=> {} {}\r\n", ep, p.display()));
        }
    }

    dirs.sort();
    files.sort();

    let mut list = String::from("# Directory Listing\r\n\r\n");
    list.push_str(&format!("Path: {}\r\n\r\n", u.path()));

    for dir in dirs {
        list.push_str(&dir);
    }
    for file in files {
        list.push_str(&file);
    }

    Ok(list)
}

// Handle CGI and return Ok(true), or indicate this request wasn't for CGI with Ok(false)
#[cfg(feature = "cgi")]
async fn handle_cgi(
    con: &mut conn::Connection,
    request: &str,
    url: &Url,
    full_path: &PathBuf,
) -> Result<bool> {
    if con.srv.server.cgi.unwrap_or(false) {
        let mut path = full_path.clone();
        let mut segments = url.path_segments().unwrap();
        let mut path_info = "".to_string();

        // Find an ancestor url that matches a file
        while !path.exists() {
            if let Some(segment) = segments.next_back() {
                path.pop();
                path_info = format!("/{}{}", &segment, path_info);
            } else {
                return Ok(false);
            }
        }
        let script_name = format!("/{}", segments.collect::<Vec<_>>().join("/"));

        let meta = tokio::fs::metadata(&path).await?;
        let perm = meta.permissions();

        match &con.srv.server.cgipath {
            Some(c) => {
                if path.starts_with(c) {
                    if perm.mode() & 0o0111 == 0o0111 {
                        cgi::cgi(con, path, url, script_name, path_info).await?;
                        return Ok(true);
                    } else {
                        logger::logger(con.peer_addr, Status::CGIError, request);
                        con.send_status(Status::CGIError, None).await?;
                        return Ok(true);
                    }
                }
            }
            None => {
                if meta.is_file() && perm.mode() & 0o0111 == 0o0111 {
                    cgi::cgi(con, path, url, script_name, path_info).await?;
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

// TODO Rewrite this monster.
pub async fn handle_connection(mut con: conn::Connection, url: url::Url) -> Result {
    let index = match &con.srv.server.index {
        Some(i) => i.clone(),
        None => "index.gemini".to_string(),
    };

    match &con.srv.server.redirect.to_owned() {
        Some(re) => {
            let u = match url.path() {
                "/" => "/",
                _ => url.path().trim_end_matches('/'),
            };
            if let Some(r) = re.get(u) {
                logger::logger(con.peer_addr, Status::RedirectTemporary, url.as_str());
                con.send_status(Status::RedirectTemporary, Some(r)).await?;
                return Ok(());
            }
        }
        None => {}
    }

    #[cfg(feature = "proxy")]
    if let Some(pr) = con.srv.server.proxy_all.to_owned() {
        let host_port: Vec<&str> = pr.splitn(2, ':').collect();
        let host = host_port[0];
        let port: Option<u16>;
        if host_port.len() == 2 {
            port = host_port[1].parse().ok();
        } else {
            port = None;
        }

        let mut upstream_url = url.clone();
        upstream_url.set_host(Some(host)).unwrap();
        upstream_url.set_port(port).unwrap();

        revproxy::proxy_all(pr.as_str(), upstream_url, con).await?;
        return Ok(());
    }

    #[cfg(feature = "proxy")]
    match &con.srv.server.proxy {
        Some(pr) => {
            if let Some(s) = url.path_segments().map(|c| c.collect::<Vec<_>>()) {
                if let Some(p) = pr.get(s[0]) {
                    revproxy::proxy(p.to_string(), url, con).await?;
                    return Ok(());
                }
            }
        }
        None => {}
    }

    #[cfg(feature = "scgi")]
    match &con.srv.server.scgi {
        Some(sc) => {
            let u = match url.path() {
                "/" => "/",
                _ => url.path().trim_end_matches('/'),
            };
            if let Some(r) = sc.get(u) {
                cgi::scgi(r.to_string(), url, con).await?;
                return Ok(());
            }
        }
        None => {}
    }

    let mut path = PathBuf::new();

    if url.path().starts_with("/~") && con.srv.server.usrdir.unwrap_or(false) {
        let usr = url.path().trim_start_matches("/~");
        let usr: Vec<&str> = usr.splitn(2, '/').collect();
        if cfg!(target_os = "macos") {
            path.push("/Users/");
        } else {
            path.push("/home/");
        }
        if usr.len() == 2 {
            path.push(format!(
                "{}/{}/{}",
                usr[0],
                "public_gemini",
                util::url_decode(usr[1].as_bytes())
            ));
        } else {
            path.push(format!("{}/{}/", usr[0], "public_gemini"));
        }
    } else {
        path.push(&con.srv.server.dir);
        if url.path() != "" || url.path() != "/" {
            let decoded = util::url_decode(url.path().trim_start_matches('/').as_bytes());
            path.push(decoded);
        }
    }

    if !path.exists() {
        // See if it's a subpath of a CGI script before returning NotFound
        #[cfg(feature = "cgi")]
        if handle_cgi(&mut con, url.as_str(), &url, &path).await? {
            return Ok(());
        }

        logger::logger(con.peer_addr, Status::NotFound, url.as_str());
        con.send_status(Status::NotFound, None).await?;
        return Ok(());
    }

    let mut meta = tokio::fs::metadata(&path).await?;
    let mut perm = meta.permissions();

    // TODO fix me
    // This block is terrible
    if meta.is_dir() {
        if !url.path().ends_with('/') {
            logger::logger(con.peer_addr, Status::RedirectPermanent, url.as_str());
            con.send_status(
                Status::RedirectPermanent,
                Some(format!("{}/", url).as_str()),
            )
            .await?;
            return Ok(());
        }
        if path.join(&index).exists() {
            path.push(index);
            meta = tokio::fs::metadata(&path).await?;
            perm = meta.permissions();
            if perm.mode() & 0o0444 != 0o444 {
                let mut p = path.clone();
                p.pop();
                path.push(format!("{}/", p.display()));
                meta = tokio::fs::metadata(&path).await?;
                perm = meta.permissions();
            }
        }
    }

    #[cfg(feature = "cgi")]
    if handle_cgi(&mut con, url.as_str(), &url, &path).await? {
        return Ok(());
    }

    if meta.is_file() && perm.mode() & 0o0111 == 0o0111 {
        logger::logger(con.peer_addr, Status::NotFound, url.as_str());
        con.send_status(Status::NotFound, None).await?;
        return Ok(());
    }

    if perm.mode() & 0o0444 != 0o0444 {
        logger::logger(con.peer_addr, Status::NotFound, url.as_str());
        con.send_status(Status::NotFound, None).await?;
        return Ok(());
    }

    let mut mime = get_mime(&path);
    if meta.is_file() {
        if mime == "text/gemini" && con.srv.server.lang.is_some() {
            mime += &("; lang=".to_string() + &con.srv.server.lang.to_owned().unwrap());
        }
        if !mime.starts_with("text/") {
            logger::logger(con.peer_addr, Status::Success, url.as_str());
            get_binary(con, path, mime).await?;
            return Ok(());
        }
        match fs::read_to_string(path).await {
            Ok(c) => {
                con.send_body(Status::Success, Some(&mime), Some(c)).await?;
                logger::logger(con.peer_addr, Status::Success, url.as_str());
            }
            Err(e) => {
                println!("{}", e);
                con.send_status(Status::NotFound, None).await?;
                logger::logger(con.peer_addr, Status::NotFound, url.as_str());
            }
        }
    } else {
        let dir = gen_dir_list(path, &url).await?;
        con.send_body(Status::Success, Some(&mime), Some(dir))
            .await?;
        logger::logger(con.peer_addr, Status::Success, url.as_str());
    }

    Ok(())
}
